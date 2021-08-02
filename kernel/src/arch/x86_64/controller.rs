use abomonation::{decode, encode};
use alloc::{vec, vec::Vec};
use log::{debug, error, trace, warn};

use smoltcp::iface::EthernetInterface;
use smoltcp::socket::{SocketHandle, SocketSet, TcpSocket, TcpSocketBuffer};
use smoltcp::time::Instant;

use vmxnet3::smoltcp::DevQueuePhy;

use rpc::rpc::{RPCError, RPCHeader, RPCType};

use crate::arch::remote_syscall::{handle_fileio, register_pid};

const RX_BUF_LEN: usize = 8192;
const TX_BUF_LEN: usize = 8192;
const PORT: u16 = 6970;

pub struct TCPServer<'a> {
    iface: EthernetInterface<'a, DevQueuePhy>,
    sockets: SocketSet<'a>,
    server_handle: Option<SocketHandle>,
}

impl TCPServer<'_> {
    pub fn new<'a>(iface: EthernetInterface<'a, DevQueuePhy>) -> TCPServer<'a> {
        TCPServer {
            iface: iface,
            sockets: SocketSet::new(vec![]),
            server_handle: None,
        }
    }

    pub fn run_server(&mut self) -> Result<(), RPCError> {
        self.init()?;
        loop {
            self.handle_rpc()?;
        }
    }

    fn init(&mut self) -> Result<(), RPCError> {
        // create server socket & start listening
        let socket_rx_buffer = TcpSocketBuffer::new(vec![0; RX_BUF_LEN]);
        let socket_tx_buffer = TcpSocketBuffer::new(vec![0; TX_BUF_LEN]);
        let mut tcp_socket = TcpSocket::new(socket_rx_buffer, socket_tx_buffer);
        tcp_socket.listen(PORT).unwrap();
        debug!("Listening at port {}", PORT);
        self.server_handle = Some(self.sockets.add(tcp_socket));

        // Wait for client to connect
        loop {
            match self.iface.poll(&mut self.sockets, Instant::from_millis(0)) {
                Ok(_) => {}
                Err(e) => {
                    warn!("poll error: {}", e);
                }
            }

            let socket = self.sockets.get::<TcpSocket>(self.server_handle.unwrap());
            if socket.is_active() && (socket.may_send() || socket.may_recv()) {
                debug!("Connected to client!");
                return Ok(());
            }
        }
    }

    fn handle_rpc(&mut self) -> Result<(), RPCError> {
        // Receive request header
        let mut req_data = self.msg_recv(core::mem::size_of::<RPCHeader>()).unwrap();
        let (hdr, extra) = unsafe { decode::<RPCHeader>(&mut req_data) }.unwrap();
        assert_eq!(extra.len(), 0);

        // Read the request payload
        let mut payload_data = Vec::new();
        if hdr.msg_len > 0 {
            payload_data = self.msg_recv(hdr.msg_len as usize).unwrap();
        }

        // If registration, update id
        if hdr.msg_type == RPCType::Registration {
            debug!("Received registration request from client: {:?}", hdr);

            // validate request
            if extra.len() != 0
                || hdr.client_id != 0
                || hdr.pid != 0
                || hdr.req_id != 0
                || hdr.msg_len != 0
                || hdr.msg_type != RPCType::Registration
            {
                error!("Invalid registration request received");
                return Err(RPCError::MalformedRequest);
            }

            // Register pid and construct response
            match register_pid(hdr.pid) {
                Ok(_) => {
                    let res = RPCHeader {
                        client_id: PORT as u64,
                        pid: 0,
                        req_id: 0,
                        msg_type: RPCType::Registration,
                        msg_len: 0,
                    };
                    let mut res_data = Vec::new();
                    unsafe { encode(&res, &mut res_data) }.unwrap();
                    return self.msg_send(res_data);
                }
                Err(err) => {
                    error!("Could not map remote pid {} to local pid {}", hdr.pid, err);
                    return Err(RPCError::InternalError);
                }
            }

        // Handle as FIO request
        } else {
            let res_data = handle_fileio(hdr, &mut payload_data);
            return self.msg_send(res_data);
        }
    }

    /// send data to a remote node
    fn msg_send(&mut self, data: Vec<u8>) -> Result<(), RPCError> {
        let mut data_sent = 0;
        loop {
            match self.iface.poll(&mut self.sockets, Instant::from_millis(0)) {
                Ok(_) => {}
                Err(e) => {
                    warn!("poll error: {}", e);
                }
            }

            if data_sent == data.len() {
                return Ok(());
            } else {
                let mut socket = self.sockets.get::<TcpSocket>(self.server_handle.unwrap());
                if socket.can_send() && socket.send_capacity() > 0 && data_sent < data.len() {
                    let end_index = data_sent + core::cmp::min(data.len(), socket.send_capacity());
                    debug!(
                        "msg_send [{:?}-{:?}] (capacity={:?}",
                        data_sent,
                        end_index,
                        socket.send_capacity()
                    );
                    if let Ok(bytes_sent) = socket.send_slice(&data[data_sent..end_index]) {
                        trace!(
                            "Client sent: [{:?}-{:?}] {:?}/{:?} bytes",
                            data_sent,
                            end_index,
                            end_index,
                            data.len()
                        );
                        data_sent = data_sent + bytes_sent;
                    } else {
                        warn!("send_slice failed... trying again?");
                    }
                }
            }
        }
    }

    /// receive data from a remote node
    fn msg_recv(&mut self, expected_data: usize) -> Result<Vec<u8>, RPCError> {
        let mut data = vec![0; expected_data];
        let mut total_data_received = 0;

        loop {
            match self.iface.poll(&mut self.sockets, Instant::from_millis(0)) {
                Ok(_) => {}
                Err(e) => {
                    warn!("poll error: {}", e);
                }
            }

            if total_data_received == expected_data {
                return Ok(data);
            } else {
                let mut socket = self.sockets.get::<TcpSocket>(self.server_handle.unwrap());
                if socket.can_recv() {
                    if let Ok(bytes_received) =
                        socket.recv_slice(&mut data[total_data_received..expected_data])
                    {
                        total_data_received += bytes_received;
                        trace!(
                            "msg_rcv got {:?}/{:?} bytes",
                            total_data_received,
                            expected_data
                        );
                    } else {
                        warn!("recv_slice failed... trying again?");
                    }
                }
            }
        }
    }
}