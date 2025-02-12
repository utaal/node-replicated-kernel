// Copyright © 2021 University of Colorado. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::vec::Vec;
use core::result::Result;

use crate::cluster_api::NodeId;
use crate::rpc::{RPCError, RPCHeader, RPCType};

/// RPC server operations
pub trait RPCServerAPI {
    /// register an RPC func with an ID
    fn register(&self, rpc_id: RPCType) -> Result<(), RPCError>;

    /// receives next RPC call with RPC ID
    fn receive(&self) -> Result<(RPCHeader, Vec<u8>), RPCError>;

    /// replies an RPC call with results
    fn reply(&self, client: NodeId, data: Vec<u8>) -> Result<(), RPCError>;

    /// Run the RPC server
    fn run_server(&mut self) -> Result<(), RPCError>;
}

/// RPC client operations
pub trait RPCClientAPI {
    /// calls a remote RPC function with ID
    fn call(&mut self, pid: usize, rpc_id: RPCType, data: Vec<u8>) -> Result<Vec<u8>, RPCError>;

    /// send data to a remote node
    fn send(&mut self, data: Vec<u8>) -> Result<(), RPCError>;

    /// receive data from a remote node
    fn recv(&mut self, expected_data: usize) -> Result<Vec<u8>, RPCError>;
}
