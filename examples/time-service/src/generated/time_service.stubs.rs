pub mod time_service {
    pub const PROGRAM: u32 = 824377345;

    pub mod time_service_v1 {
        pub const VERSION: u32 = 1;
        pub const GET_TIME: u32 = 1;

        pub mod client {
            pub struct TIME_SERVICE_V1Client<T> {
                client: onc_rpc_runtime::Client<T>,
            }

            impl<T> TIME_SERVICE_V1Client<T> where T: onc_rpc_runtime::ClientTransport {
                pub fn new(client: onc_rpc_runtime::Client<T>) -> Self {
                    Self { client }
                }

                pub fn get_time(&self) -> Result<crate::time_service::time_string, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::GET_TIME),
                        &(),
                    )
                }

                pub fn get_time_with_options(&self, options: &onc_rpc_runtime::CallOptions) -> Result<crate::time_service::time_string, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed_with_options(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::GET_TIME),
                        &(),
                        options,
                    )
                }

            }

        }

        pub mod async_client {
            pub struct TIME_SERVICE_V1Client<T> {
                client: onc_rpc_runtime::AsyncClient<T>,
            }

            impl<T> TIME_SERVICE_V1Client<T> where T: onc_rpc_runtime::AsyncClientTransport {
                pub fn new(client: onc_rpc_runtime::AsyncClient<T>) -> Self {
                    Self { client }
                }

                pub async fn get_time(&self) -> Result<crate::time_service::time_string, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::GET_TIME),
                        &(),
                    ).await
                }

                pub async fn get_time_with_options(&self, options: &onc_rpc_runtime::CallOptions) -> Result<crate::time_service::time_string, onc_rpc_runtime::RuntimeError> {
                    self.client.call_typed_with_options(
                        onc_rpc_runtime::ProgramVersion { program: super::super::PROGRAM, version: super::VERSION },
                        onc_rpc_runtime::Procedure(super::GET_TIME),
                        &(),
                        options,
                    ).await
                }

            }

        }

        pub mod server {
            pub trait TIME_SERVICE_V1Service {
                fn get_time(&self, request: &onc_rpc_server::RequestContext) -> Result<crate::time_service::time_string, onc_rpc_server::DispatchError>;
            }

            pub struct TIME_SERVICE_V1Dispatch<T> {
                inner: T,
            }

            impl<T> TIME_SERVICE_V1Dispatch<T> {
                pub fn new(inner: T) -> Self {
                    Self { inner }
                }
            }

            impl<T> onc_rpc_server::Dispatch for TIME_SERVICE_V1Dispatch<T> where T: TIME_SERVICE_V1Service + Send + Sync + 'static {
                fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {
                    match request.procedure.0 {
                        1 => {
                            <() as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload).map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;
                            let response = self.inner.get_time(&request)?;
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
            pub trait TIME_SERVICE_V1Service {
                async fn get_time(&self, request: &onc_rpc_server::RequestContext) -> Result<crate::time_service::time_string, onc_rpc_server::DispatchError>;
            }

            pub struct TIME_SERVICE_V1Dispatch<T> {
                inner: T,
            }

            impl<T> TIME_SERVICE_V1Dispatch<T> {
                pub fn new(inner: T) -> Self {
                    Self { inner }
                }
            }

            #[onc_rpc_server::async_trait]
            impl<T> onc_rpc_server::AsyncDispatch for TIME_SERVICE_V1Dispatch<T> where T: TIME_SERVICE_V1Service + Send + Sync + 'static {
                async fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {
                    match request.procedure.0 {
                        1 => {
                            <() as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload).map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;
                            let response = self.inner.get_time(&request).await?;
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

