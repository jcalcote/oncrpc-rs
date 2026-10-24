pub mod buffer_echo {
    pub const PROGRAM: u32 = 824377346;

    pub mod buffer_echo_v1 {
        pub const VERSION: u32 = 1;
        pub const ECHO: u32 = 1;

        pub mod client {
            pub struct BUFFER_ECHO_V1Client<T> {
                client: onc_rpc_runtime::Client<T>,
            }

            impl<T> BUFFER_ECHO_V1Client<T> where T: onc_rpc_runtime::ClientTransport {
                pub fn new(client: onc_rpc_runtime::Client<T>) -> Self {
                    Self { client }
                }

                /// Call RPC `ECHO` and decode its typed reply.
                /// Blocks until completion using the client's default call policy.
                pub fn echo(&self, argument: crate::buffer_echo::echo_data) -> Result<crate::buffer_echo::echo_data, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::ECHO),
                        &argument,
                    )
                }

                /// Call RPC `ECHO` with explicit call options.
                /// Options select timeout, authentication and local cancellation policy.
                pub fn echo_with_options(&self, argument: crate::buffer_echo::echo_data, options: &onc_rpc_runtime::CallOptions) -> Result<crate::buffer_echo::echo_data, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed_with_options(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::ECHO),
                        &argument,
                        options,
                    )
                }

            }

        }

        pub mod async_client {
            pub struct BUFFER_ECHO_V1Client<T> {
                client: onc_rpc_runtime::AsyncClient<T>,
            }

            impl<T> BUFFER_ECHO_V1Client<T> where T: onc_rpc_runtime::AsyncClientTransport {
                pub fn new(client: onc_rpc_runtime::AsyncClient<T>) -> Self {
                    Self { client }
                }

                /// Call RPC `ECHO` and decode its typed reply.
                /// Awaits completion using the client's default call policy.
                pub async fn echo(&self, argument: crate::buffer_echo::echo_data) -> Result<crate::buffer_echo::echo_data, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::ECHO),
                        &argument,
                    ).await
                }

                /// Call RPC `ECHO` with explicit call options.
                /// Options select timeout, authentication and local cancellation policy.
                pub async fn echo_with_options(&self, argument: crate::buffer_echo::echo_data, options: &onc_rpc_runtime::CallOptions) -> Result<crate::buffer_echo::echo_data, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed_with_options(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::ECHO),
                        &argument,
                        options,
                    ).await
                }

            }

            impl BUFFER_ECHO_V1Client<onc_rpc_runtime::OwnedRpcTransport> {
                /// Call RPC `ECHO` with segmented encoding and a caller-owned reply buffer.
                /// Decodes the full typed reply; the outcome returns `buffer`, including on error.
                pub async fn echo_segmented<B: AsMut<[u8]> + Send + 'static, A: Into<crate::buffer_echo::echo_dataSegments>>(&self, argument: A, buffer: B, options: &onc_rpc_runtime::CallOptions) -> onc_rpc_runtime::TypedReceiveOutcome<crate::buffer_echo::echo_data, B> {
                    let argument: crate::buffer_echo::echo_dataSegments = argument.into();
                    self.client.call_xdr_segmented::<_, crate::buffer_echo::echo_data, B>(onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION }, onc_rpc_runtime::Procedure(super::ECHO), &argument, buffer, options).await
                }
                /// Admit RPC `ECHO` with segmented encoding, without waiting for capacity.
                /// Returns an unpolled future, or the unsent request and buffer on refusal.
                #[allow(clippy::result_large_err)] // Refusal returns original owners without boxing.
                pub fn try_echo_segmented<B: AsMut<[u8]> + Send + 'static, A: Into<crate::buffer_echo::echo_dataSegments>>(&self, argument: A, buffer: B, options: &onc_rpc_runtime::CallOptions) -> Result<impl std::future::Future<Output = onc_rpc_runtime::TypedReceiveOutcome<crate::buffer_echo::echo_data, B>> + Send + 'static + use<A, B>, onc_rpc_runtime::CallRefusal<crate::buffer_echo::echo_dataSegments, B>> {
                    let argument: crate::buffer_echo::echo_dataSegments = argument.into();
                    self.client.try_call_xdr_segmented::<_, crate::buffer_echo::echo_data, B>(onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION }, onc_rpc_runtime::Procedure(super::ECHO), argument, buffer, options)
                }
                /// Call RPC `ECHO`, placing the reply's opaque body directly in `buffer`.
                /// Returns typed metadata and the quiescent buffer, including on error.
                pub async fn echo_into<B: AsMut<[u8]> + Send + 'static, A: Into<crate::buffer_echo::echo_dataSegments>>(&self, argument: A, buffer: B, options: &onc_rpc_runtime::CallOptions) -> onc_rpc_runtime::TypedReceiveOutcome<crate::buffer_echo::echo_dataMetadata, B> {
                    let argument: crate::buffer_echo::echo_dataSegments = argument.into();
                    self.client.call_xdr_into::<_, crate::buffer_echo::echo_dataMetadata, B>(onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION }, onc_rpc_runtime::Procedure(super::ECHO), &argument, buffer, options).await
                }
                /// Admit RPC `ECHO` for direct-body receive, without waiting for capacity.
                /// Returns an unpolled future, or the unsent request and buffer on refusal.
                #[allow(clippy::result_large_err)] // Refusal returns original owners without boxing.
                pub fn try_echo_into<B: AsMut<[u8]> + Send + 'static, A: Into<crate::buffer_echo::echo_dataSegments>>(&self, argument: A, buffer: B, options: &onc_rpc_runtime::CallOptions) -> Result<impl std::future::Future<Output = onc_rpc_runtime::TypedReceiveOutcome<crate::buffer_echo::echo_dataMetadata, B>> + Send + 'static + use<A, B>, onc_rpc_runtime::CallRefusal<crate::buffer_echo::echo_dataSegments, B>> {
                    let argument: crate::buffer_echo::echo_dataSegments = argument.into();
                    self.client.try_call_xdr_into::<_, crate::buffer_echo::echo_dataMetadata, B>(onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION }, onc_rpc_runtime::Procedure(super::ECHO), argument, buffer, options)
                }

            }

        }

        pub mod server {
            pub trait BUFFER_ECHO_V1Service {
                /// Handle RPC `ECHO` and return its typed reply.
                /// Request metadata is available through `request`.
                fn echo(&self, request: &onc_rpc_server::RequestContext, argument: crate::buffer_echo::echo_data) -> Result<crate::buffer_echo::echo_data, onc_rpc_server::DispatchError>;
            }

            pub struct BUFFER_ECHO_V1Dispatch<T> {
                inner: T,
            }

            impl<T> BUFFER_ECHO_V1Dispatch<T> {
                pub fn new(inner: T) -> Self {
                    Self { inner }
                }
            }

            impl<T> onc_rpc_server::Dispatch for BUFFER_ECHO_V1Dispatch<T> where T: BUFFER_ECHO_V1Service + Send + Sync + 'static {
                fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {
                    match request.procedure.0 {
                        1 => {
                            let argument = <crate::buffer_echo::echo_data as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload).map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;
                            let response = self.inner.echo(&request, argument)?;
                            let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&response).map_err(|_| onc_rpc_server::DispatchError::SystemError)?;
                            Ok(onc_rpc_server::ResponsePayload::success(payload))
                        }
                        _ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),
                    }
                }
            }

        }

        pub mod async_server {
            #[onc_rpc_server::async_trait]
            pub trait BUFFER_ECHO_V1Service {
                /// Handle RPC `ECHO` and return its typed reply.
                /// Request metadata is available through `request`.
                async fn echo(&self, request: &onc_rpc_server::RequestContext, argument: crate::buffer_echo::echo_data) -> Result<crate::buffer_echo::echo_data, onc_rpc_server::DispatchError>;
            }

            pub struct BUFFER_ECHO_V1Dispatch<T> {
                inner: T,
            }

            impl<T> BUFFER_ECHO_V1Dispatch<T> {
                pub fn new(inner: T) -> Self {
                    Self { inner }
                }
            }

            #[onc_rpc_server::async_trait]
            impl<T> onc_rpc_server::AsyncDispatch for BUFFER_ECHO_V1Dispatch<T> where T: BUFFER_ECHO_V1Service + Send + Sync + 'static {
                async fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {
                    match request.procedure.0 {
                        1 => {
                            let argument = <crate::buffer_echo::echo_data as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload).map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;
                            let response = self.inner.echo(&request, argument).await?;
                            let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&response).map_err(|_| onc_rpc_server::DispatchError::SystemError)?;
                            Ok(onc_rpc_server::ResponsePayload::success(payload))
                        }
                        _ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),
                    }
                }
            }

        }

    }

}
