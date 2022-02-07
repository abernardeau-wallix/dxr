use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use axum::http::{header::CONTENT_LENGTH, header::CONTENT_TYPE, HeaderMap, HeaderValue, StatusCode};
use axum::routing::post;
use axum::Router;

use dxr_shared::{DxrError, Fault, FaultResponse, MethodCall, MethodResponse, Value};

use crate::handler::Handler;

/// builder that takes parameters for constructing a [`Server`]
pub struct ServerBuilder {
    addr: SocketAddr,
    handlers: HashMap<&'static str, RwLock<Box<dyn Handler + Send + Sync>>>,
}

impl Debug for ServerBuilder {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut handler_list: Vec<&&str> = self.handlers.keys().collect();
        handler_list.sort();

        f.debug_struct("ServerBuilder")
            .field("addr", &self.addr)
            .field("handlers", &handler_list)
            .finish()
    }
}

impl ServerBuilder {
    /// constructor for [`ServerBuilder`] from the address of the XML-RPC server
    pub fn new(addr: SocketAddr) -> ServerBuilder {
        ServerBuilder {
            addr,
            handlers: HashMap::new(),
        }
    }

    /// method for adding a new method handler for the [`Server`]
    pub fn add_method(mut self, name: &'static str, handler: Box<dyn Handler + Send + Sync>) -> Self {
        self.handlers.insert(name, RwLock::new(handler));
        self
    }

    /// build the [`Server`] from the specified URL and registered method handlers
    pub fn build(self) -> Server {
        Server {
            addr: self.addr,
            handlers: Arc::new(self.handlers),
        }
    }
}

/// # XML-RPC server implementation
///
/// This type provides a very simple XML-RPC server implementation. Specify server address,
/// register method handlers, initialize the [`Server`], and wait for requests.
pub struct Server {
    addr: SocketAddr,
    handlers: Arc<HashMap<&'static str, RwLock<Box<dyn Handler + Send + Sync>>>>,
}

impl Debug for Server {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut handler_list: Vec<&&str> = self.handlers.keys().collect();
        handler_list.sort();

        f.debug_struct("ServerBuilder")
            .field("addr", &self.addr)
            .field("handlers", &handler_list)
            .finish()
    }
}

impl Server {
    /// asynchronous method for processing remote procedure calls via XML-RPC
    ///
    /// Requests with invalid input, calls of unknown methods, and failed methods are converted
    /// into fault responses.
    pub async fn serve(self) -> Result<(), String> {
        let app = Router::new().route(
            "/",
            post({
                move |body: String, headers: HeaderMap| async move {
                    if headers.get(CONTENT_LENGTH).is_none() {
                        return fault_to_response(411, "Content-Length header missing.");
                    }

                    let call: MethodCall = match quick_xml::de::from_str(&body) {
                        Ok(call) => call,
                        Err(error) => {
                            let e = DxrError::invalid_data(error.to_string());
                            let f = Fault::new(400, e.to_string());
                            return fault_to_response(f.code(), f.string());
                        },
                    };

                    let mut handler = match self.handlers.get(call.name()) {
                        Some(handler) => handler.write().expect("Poisoned lock!"),
                        None => return fault_to_response(404, "Unknown method."),
                    };

                    let response = match handler.handle(call.params(), &headers) {
                        Ok(value) => success_to_response(value),
                        Err(fault) => fault_to_response(fault.code(), fault.string()),
                    };

                    response
                }
            }),
        );

        axum::Server::bind(&self.addr)
            .serve(app.into_make_service())
            .await
            .map_err(|error| error.to_string())
    }
}

fn response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/xml"));
    headers
}

fn success_to_response(value: Value) -> (StatusCode, HeaderMap, String) {
    let response = MethodResponse::new(value);

    match quick_xml::se::to_string(&response) {
        Ok(success) => (StatusCode::OK, response_headers(), success),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, response_headers(), error.to_string()),
    }
}

fn fault_to_response(code: i32, string: &str) -> (StatusCode, HeaderMap, String) {
    let fault = Fault::new(code, string.to_owned());
    let response: FaultResponse = fault.into();

    match quick_xml::se::to_string(&response) {
        Ok(fault) => (StatusCode::OK, response_headers(), fault),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, response_headers(), error.to_string()),
    }
}
