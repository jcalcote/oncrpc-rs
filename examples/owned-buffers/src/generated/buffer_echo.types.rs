use bytes::Bytes;

pub const ECHO_MAX: i64 = 64;

pub type echo_data = Bytes;

pub mod buffer_echo {
    pub const PROGRAM: u32 = 824377346;

    pub mod buffer_echo_v1 {
        pub const VERSION: u32 = 1;
        pub const ECHO: u32 = 1;
    }

}

/// Metadata-only projection; the terminal opaque body remains in caller storage.
pub type echo_dataMetadata = onc_rpc_xdr::OpaqueLength<{ ECHO_MAX as usize }>;

/// Alternate argument representation retaining multiple owners for one terminal opaque body.
pub type echo_dataSegments = onc_rpc_xdr::OpaqueSegments<{ ECHO_MAX as usize }>;
