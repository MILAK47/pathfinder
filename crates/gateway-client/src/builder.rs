//! Provides a builder API for creating and sending Sequencer REST requests.
//!
//! This builder utilises a type state builder pattern with generics to only allow valid operations at each stage of the build process.
//! Each stage is consumed to generate the next stage and the final stage executes the query.
//!
//! Here is an overview of the four builder stages.
//!
//!   1. [Init](stage::Init) which provides the entry point of the [builder](Request).
//!   2. [Method](stage::Method) where you select the REST API method.
//!   3. [Params](stage::Params) where you select the retry behavior.
//!   4. [Final](stage::Final) where you select the REST operation type, which is then executed.
use crate::metrics::{with_metrics, BlockTag, RequestMetadata};
use pathfinder_common::{BlockId, ClassHash, TransactionHash};
use starknet_gateway_types::error::SequencerError;

const X_THROTTLING_BYPASS: &str = "X-Throttling-Bypass";

/// A Sequencer Request builder.
pub struct Request<'a, S: RequestState> {
    state: S,
    url: reqwest::Url,
    api_key: Option<String>,
    client: &'a reqwest::Client,
}

pub mod stage {
    use crate::metrics::RequestMetadata;

    /// Provides the [builder](super::Request::builder) entry-point.
    pub struct Init;

    /// Select the Sequencer API method to call:
    /// - [add_transaction](super::Request::add_transaction)
    /// - [get_block](super::Request::get_block)
    /// - [get_class_by_hash](super::Request::get_class_by_hash)
    /// - [get_transaction](super::Request::get_transaction)
    /// - [get_state_update](super::Request::get_state_update)
    /// - [get_contract_addresses](super::Request::get_contract_addresses)
    pub struct Method;

    /// Specify the request parameters:
    /// - [at_block](super::Request::with_block)
    /// - [with_class_hash](super::Request::with_class_hash)
    /// - [with_optional_token](super::Request::with_optional_token)
    /// - [with_transaction_hash](super::Request::with_transaction_hash)
    /// - [add_param](super::Request::add_param) (allows adding custom (name, value) parameter)
    ///
    /// and then specify the [retry behavior](super::Request::with_retry).
    pub struct Params {
        pub meta: RequestMetadata,
    }

    /// Specify the REST operation send the request:
    /// - [get](super::Request::get)
    /// - [get_as_bytes](super::Request::get_as_bytes)
    /// - [post_with_json](super::Request::post_with_json)
    pub struct Final {
        pub meta: RequestMetadata,
        pub retry: bool,
    }

    impl super::RequestState for Init {}
    impl super::RequestState for Method {}
    impl super::RequestState for Params {}
    impl super::RequestState for Final {}
}

impl<'a> Request<'a, stage::Init> {
    /// Initialize a [Request] builder.
    pub fn builder(
        client: &'a reqwest::Client,
        url: reqwest::Url,
        api_key: Option<String>,
    ) -> Request<'a, stage::Method> {
        Request {
            url,
            client,
            api_key,
            state: stage::Method,
        }
    }
}

/// Helper macros used in [`stage::Method`]
mod request_macros {
    /// Generates the const `METHODS` slice. At least one item is required.
    macro_rules! method_names {
        () => {
            compile_error!("At least one method has to be defined");
        };
        ($($x:ident),+ $(,)?) => {
            pub const METHODS: &'static [&'static str] = &[$(stringify!($x)),+];
        };
    }

    /// Generates methods with names from the list.
    ///
    /// Each generated method delegates the call to `with_method`.
    macro_rules! method_defs {
        ($($x:ident),+ $(,)?) => {
            $(request_macros::method!($x);)+
        };
    }

    /// Generates one method with `name`.
    ///
    /// The generated method delegates the call to `with_method`.
    macro_rules! method {
        ($name:ident) => {
            pub fn $name(self) -> Request<'a, stage::Params> {
                self.with_method(stringify!($name))
            }
        };
    }

    /// Generates methods with names from the list and a const slice `METHODS`
    /// which then can be used to register metrics per method.
    macro_rules! methods {
        () => {
            method_names!();
        };
        ($($x:ident),+ $(,)?) => {
            request_macros::method_names!($($x),+);
            $(request_macros::method_defs!($x);)+
        };
    }

    pub(super) use {method, method_defs, method_names, methods};
}

impl<'a> Request<'a, stage::Method> {
    request_macros::methods!(
        add_transaction,
        get_block,
        get_class_by_hash,
        get_compiled_class_by_class_hash,
        get_transaction,
        get_state_update,
        get_contract_addresses,
        get_block_traces,
        get_transaction_trace,
        get_signature,
    );

    /// Appends the given method to the request url.
    fn with_method(mut self, method: &'static str) -> Request<'a, stage::Params> {
        self.url
            .path_segments_mut()
            .expect("Base URL is valid")
            .push(method);

        Request {
            url: self.url,
            client: self.client,
            api_key: self.api_key,
            state: stage::Params {
                meta: RequestMetadata::new(method),
            },
        }
    }
}

impl<'a> Request<'a, stage::Params> {
    pub fn with_block<B: Into<BlockId>>(self, block: B) -> Self {
        use std::borrow::Cow;

        let block: BlockId = block.into();
        let (name, value, tag) = match block {
            BlockId::Number(number) => (
                "blockNumber",
                Cow::from(number.get().to_string()),
                BlockTag::None,
            ),
            BlockId::Hash(hash) => ("blockHash", hash.0.to_hex_str(), BlockTag::None),
            // These have to use "blockNumber", "blockHash" does not accept tags.
            BlockId::Latest => ("blockNumber", Cow::from("latest"), BlockTag::Latest),
            BlockId::Pending => ("blockNumber", Cow::from("pending"), BlockTag::Pending),
        };

        self.update_tag(tag).add_param(name, &value)
    }

    pub fn with_class_hash(self, class_hash: ClassHash) -> Self {
        self.add_param("classHash", &class_hash.0.to_hex_str())
    }

    pub fn with_optional_token(self, token: Option<&str>) -> Self {
        match token {
            Some(token) => self.add_param("token", token),
            None => self,
        }
    }

    pub fn with_transaction_hash(self, hash: TransactionHash) -> Self {
        self.add_param("transactionHash", &hash.0.to_hex_str())
    }

    pub fn add_param(mut self, name: &str, value: &str) -> Self {
        self.url.query_pairs_mut().append_pair(name, value);
        self
    }

    pub fn update_tag(mut self, tag: BlockTag) -> Self {
        self.state.meta.tag = tag;
        self
    }

    /// Sets the request retry behavior.
    pub fn with_retry(self, retry: bool) -> Request<'a, stage::Final> {
        Request {
            url: self.url,
            client: self.client,
            api_key: self.api_key,
            state: stage::Final {
                meta: self.state.meta,
                retry,
            },
        }
    }
}

impl<'a> Request<'a, stage::Final> {
    /// Sends the Sequencer request as a REST `GET` operation and parses the response into `T`.
    pub async fn get<T>(self) -> Result<T, SequencerError>
    where
        T: serde::de::DeserializeOwned,
    {
        async fn send_request<T: serde::de::DeserializeOwned>(
            url: reqwest::Url,
            api_key: Option<String>,
            client: &reqwest::Client,
            meta: RequestMetadata,
        ) -> Result<T, SequencerError> {
            with_metrics(meta, async move {
                tracing::trace!(%url, "Fetching data from feeder gateway");
                let request = client.get(url);
                let request = match api_key {
                    Some(api_key) => request.header(X_THROTTLING_BYPASS, api_key),
                    None => request,
                };
                let response = request.send().await?;
                parse::<T>(response).await
            })
            .await
        }

        match self.state.retry {
            false => send_request(self.url, self.api_key, self.client, self.state.meta).await,
            true => {
                retry0(
                    || async {
                        let url = self.url.clone();
                        let api_key = self.api_key.clone();
                        send_request(url, api_key, self.client, self.state.meta).await
                    },
                    retry_condition,
                )
                .await
            }
        }
    }

    /// Sends the Sequencer request as a REST `GET` operation and returns the response's bytes.
    pub async fn get_as_bytes(self) -> Result<bytes::Bytes, SequencerError> {
        async fn get_as_bytes_inner(
            url: reqwest::Url,
            api_key: Option<String>,
            client: &reqwest::Client,
            meta: RequestMetadata,
        ) -> Result<bytes::Bytes, SequencerError> {
            with_metrics(meta, async {
                tracing::trace!(%url, "Fetching binary data from feeder gateway");
                let request = client.get(url);
                let request = match api_key {
                    Some(api_key) => request.header(X_THROTTLING_BYPASS, api_key),
                    None => request,
                };
                let response = request.send().await?;
                let response = parse_raw(response).await?;
                let bytes = response.bytes().await?;
                Ok(bytes)
            })
            .await
        }

        match self.state.retry {
            false => get_as_bytes_inner(self.url, self.api_key, self.client, self.state.meta).await,
            true => {
                retry0(
                    || async {
                        let url = self.url.clone();
                        let api_key = self.api_key.clone();
                        get_as_bytes_inner(url, api_key, self.client, self.state.meta).await
                    },
                    retry_condition,
                )
                .await
            }
        }
    }

    /// Sends the Sequencer request as a REST `POST` operation, in addition to the specified
    /// JSON body. The response is parsed as type `T`.
    pub async fn post_with_json<T, J>(self, json: &J) -> Result<T, SequencerError>
    where
        T: serde::de::DeserializeOwned,
        J: serde::Serialize + ?Sized,
    {
        async fn post_with_json_inner<T, J>(
            url: reqwest::Url,
            api_key: Option<String>,
            client: &reqwest::Client,
            meta: RequestMetadata,
            json: &J,
        ) -> Result<T, SequencerError>
        where
            T: serde::de::DeserializeOwned,
            J: serde::Serialize + ?Sized,
        {
            with_metrics(meta, async {
                let request = client.post(url);
                let request = match api_key {
                    Some(api_key) => request.header(X_THROTTLING_BYPASS, api_key),
                    None => request,
                };
                let response = request.json(json).send().await?;
                parse::<T>(response).await
            })
            .await
        }

        match self.state.retry {
            false => {
                post_with_json_inner(self.url, self.api_key, self.client, self.state.meta, json)
                    .await
            }
            true => {
                retry0(
                    || async {
                        tracing::trace!(url=%self.url, "Posting data to gateway");
                        let url = self.url.clone();
                        let api_key = self.api_key.clone();
                        post_with_json_inner(url, api_key, self.client, self.state.meta, json).await
                    },
                    retry_condition,
                )
                .await
            }
        }
    }
}

async fn parse<T>(response: reqwest::Response) -> Result<T, SequencerError>
where
    T: ::serde::de::DeserializeOwned,
{
    let response = parse_raw(response).await?;
    // Attempt to deserialize the actual data we are looking for
    let response = response.json::<T>().await?;
    Ok(response)
}

/// Helper function which allows skipping deserialization when required.
async fn parse_raw(response: reqwest::Response) -> Result<reqwest::Response, SequencerError> {
    use starknet_gateway_types::error::StarknetError;

    // Starknet specific errors end with a 400 or 500 status code
    // but the body contains a JSON object with the error description
    if response.status() == reqwest::StatusCode::INTERNAL_SERVER_ERROR
        || response.status() == reqwest::StatusCode::BAD_REQUEST
    {
        let error = match response.json::<StarknetError>().await {
            Ok(e) => SequencerError::StarknetError(e),
            Err(e) if e.is_decode() => SequencerError::InvalidStarknetErrorVariant,
            Err(e) => SequencerError::ReqwestError(e),
        };
        return Err(error);
    }
    // Status codes 401..499 and 501..599 are mapped to SequencerError::TransportError
    response.error_for_status_ref().map(|_| ())?;
    Ok(response)
}

pub trait RequestState {}

/// Wrapper function to allow retrying sequencer queries in an exponential manner.
async fn retry0<T, Fut, FutureFactory, Ret>(
    future_factory: FutureFactory,
    retry_condition: Ret,
) -> Result<T, SequencerError>
where
    Fut: futures::Future<Output = Result<T, SequencerError>>,
    FutureFactory: FnMut() -> Fut,
    Ret: FnMut(&SequencerError) -> bool,
{
    use pathfinder_retry::Retry;
    use std::num::NonZeroU64;

    Retry::exponential(future_factory, NonZeroU64::new(2).unwrap())
        .factor(NonZeroU64::new(15).unwrap())
        .max_delay(std::time::Duration::from_secs(10 * 60))
        .when(retry_condition)
        .await
}

/// Determines if an error is retryable or not.
fn retry_condition(e: &SequencerError) -> bool {
    use reqwest::StatusCode;
    use tracing::{debug, error, info, warn};

    match e {
        SequencerError::ReqwestError(e) => {
            if e.is_body() || e.is_connect() || e.is_timeout() {
                info!(reason=%e, "Request failed, retrying");
            } else if e.is_status() {
                match e.status() {
                    Some(
                        StatusCode::NOT_FOUND
                        | StatusCode::TOO_MANY_REQUESTS
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT,
                    ) => {
                        debug!(reason=%e, "Request failed, retrying");
                    }
                    Some(StatusCode::INTERNAL_SERVER_ERROR) => {
                        error!(reason=%e, "Request failed, retrying");
                    }
                    Some(_) => warn!(reason=%e, "Request failed, retrying"),
                    None => unreachable!(),
                }
            } else if e.is_decode() {
                error!(reason=%e, "Request failed, retrying");
            } else {
                warn!(reason=%e, "Request failed, retrying");
            }

            true
        }
        SequencerError::StarknetError(_) => false,
        SequencerError::InvalidStarknetErrorVariant => {
            error!(reason=%e, "Request failed, retrying");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    mod retry {
        use assert_matches::assert_matches;
        use http::{response::Builder, StatusCode};
        use pretty_assertions_sorted::assert_eq;
        use std::{collections::VecDeque, net::SocketAddr, sync::Arc, time::Duration};
        use tokio::{sync::Mutex, task::JoinHandle};
        use warp::Filter;

        use crate::builder::{retry0, retry_condition};

        // A test helper
        fn status_queue_server(
            statuses: VecDeque<(StatusCode, &'static str)>,
        ) -> (JoinHandle<()>, SocketAddr) {
            use std::cell::RefCell;

            let statuses = Arc::new(Mutex::new(RefCell::new(statuses)));
            let any = warp::any().then(move || {
                let s = statuses.clone();
                async move {
                    let s = s.lock().await;
                    let s = s.borrow_mut().pop_front().unwrap();
                    Builder::new().status(s.0).body(s.1)
                }
            });

            let (addr, run_srv) = warp::serve(any).bind_ephemeral(([127, 0, 0, 1], 0));
            let server_handle = tokio::spawn(run_srv);
            (server_handle, addr)
        }

        // A test helper
        fn slow_server() -> (tokio::task::JoinHandle<()>, std::net::SocketAddr) {
            let any = warp::any().then(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Builder::new().status(200).body(""))
            });
            let (addr, run_srv) = warp::serve(any).bind_ephemeral(([127, 0, 0, 1], 0));
            let server_handle = tokio::spawn(run_srv);
            (server_handle, addr)
        }

        #[test_log::test(tokio::test)]
        async fn stop_on_ok() {
            use crate::builder;

            tokio::time::pause();

            let statuses = VecDeque::from([
                (StatusCode::TOO_MANY_REQUESTS, ""),
                (StatusCode::BAD_GATEWAY, ""),
                (StatusCode::SERVICE_UNAVAILABLE, ""),
                (StatusCode::GATEWAY_TIMEOUT, ""),
                (StatusCode::OK, r#""Finally!""#),
                (StatusCode::TOO_MANY_REQUESTS, ""),
                (StatusCode::BAD_GATEWAY, ""),
                (StatusCode::SERVICE_UNAVAILABLE, ""),
            ]);

            let (_jh, addr) = status_queue_server(statuses);
            let result = retry0(
                || async {
                    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
                    url.set_port(Some(addr.port())).unwrap();
                    let response = reqwest::get(url).await?;
                    builder::parse::<String>(response).await
                },
                retry_condition,
            )
            .await
            .unwrap();
            assert_eq!(result, "Finally!");
        }

        #[test_log::test(tokio::test)]
        async fn stop_on_fatal() {
            use crate::builder;
            use starknet_gateway_types::error::{KnownStarknetErrorCode, SequencerError};

            tokio::time::pause();

            let statuses = VecDeque::from([
                (StatusCode::TOO_MANY_REQUESTS, ""),
                (StatusCode::BAD_GATEWAY, ""),
                (StatusCode::SERVICE_UNAVAILABLE, ""),
                (StatusCode::GATEWAY_TIMEOUT, ""),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    r#"{"code":"StarknetErrorCode.BLOCK_NOT_FOUND","message":""}"#,
                ),
                (StatusCode::TOO_MANY_REQUESTS, ""),
                (StatusCode::BAD_GATEWAY, ""),
                (StatusCode::SERVICE_UNAVAILABLE, ""),
            ]);

            let (_jh, addr) = status_queue_server(statuses);
            let error = retry0(
                || async {
                    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
                    url.set_port(Some(addr.port())).unwrap();
                    let response = reqwest::get(url).await?;
                    builder::parse::<String>(response).await
                },
                retry_condition,
            )
            .await
            .unwrap_err();
            assert_matches!(
                error,
                SequencerError::StarknetError(se) => assert_eq!(se.code, KnownStarknetErrorCode::BlockNotFound.into())
            );
        }

        #[tokio::test(flavor = "current_thread")]
        async fn request_timeout() {
            use crate::builder;

            use std::sync::atomic::{AtomicUsize, Ordering};

            tokio::time::pause();

            let (_jh, addr) = slow_server();
            static CNT: AtomicUsize = AtomicUsize::new(0);

            let fut = retry0(
                || async {
                    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
                    url.set_port(Some(addr.port())).unwrap();

                    let client = reqwest::Client::builder().build().unwrap();

                    CNT.fetch_add(1, Ordering::Relaxed);

                    // This is the same as using Client::builder().timeout()
                    let response = client
                        .get(url)
                        .timeout(Duration::from_millis(1))
                        .send()
                        .await?;
                    builder::parse::<String>(response).await
                },
                retry_condition,
            );

            // The retry loops forever, so wrap it in a timeout and check the counter.
            // 5 retries = 465s
            // 6 retries = 945s
            tokio::time::timeout(Duration::from_secs(500), fut)
                .await
                .unwrap_err();

            // 5th try should have timedout if this is really exponential backoff
            assert_eq!(CNT.load(Ordering::Relaxed), 5);
        }
    }

    mod invalid_starknet_error_variant {
        use crate::Client;
        use http::response::Builder;
        use warp::Filter;

        fn server() -> (tokio::task::JoinHandle<()>, std::net::SocketAddr) {
            let any = warp::any().then(|| async { Builder::new().status(500).body("whatever") });
            let (addr, run_srv) = warp::serve(any).bind_ephemeral(([127, 0, 0, 1], 0));
            let server_handle = tokio::spawn(run_srv);
            (server_handle, addr)
        }

        #[tokio::test]
        async fn causes_short_reply() {
            let (_jh, addr) = server();
            let mut url = reqwest::Url::parse("http://localhost/").unwrap();
            url.set_port(Some(addr.port())).unwrap();
            let client = Client::with_base_url(url)
                .unwrap()
                .disable_retry_for_tests();
            let error = client.chain().await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "error decoding response body: invalid error variant"
            );
        }
    }

    mod api_key_is_set_when_configured {
        use crate::Client;
        use fake::{Fake, Faker};
        use httpmock::{prelude::*, Mock};
        use serde_json::json;

        async fn setup_with_fake_api_key(server: &MockServer) -> (Mock<'_>, Client) {
            let api_key = Faker.fake::<String>();

            let mock = server.mock(|when, then| {
                when.any_request().header("X-Throttling-Bypass", &api_key);
                then.status(200).json_body(json!({}));
            });

            let client = Client::with_base_url(server.base_url().parse().unwrap())
                .unwrap()
                .with_api_key(Some(api_key.clone()));

            (mock, client)
        }

        #[tokio::test]
        async fn get() -> anyhow::Result<()> {
            let server = MockServer::start_async().await;
            let (mock, client) = setup_with_fake_api_key(&server).await;

            let _: serde_json::Value = client
                .clone()
                .gateway_request()
                .with_method("")
                .with_retry(false)
                .get()
                .await?;

            let _: serde_json::Value = client
                .clone()
                .feeder_gateway_request()
                .with_method("")
                .with_retry(false)
                .get()
                .await?;

            mock.assert_hits(2);

            Ok(())
        }

        #[tokio::test]
        async fn get_as_bytes() -> anyhow::Result<()> {
            let server = MockServer::start_async().await;
            let (mock, client) = setup_with_fake_api_key(&server).await;

            let _: bytes::Bytes = client
                .clone()
                .gateway_request()
                .with_method("")
                .with_retry(false)
                .get_as_bytes()
                .await?;

            let _: bytes::Bytes = client
                .clone()
                .feeder_gateway_request()
                .with_method("")
                .with_retry(false)
                .get_as_bytes()
                .await?;

            mock.assert_hits(2);

            Ok(())
        }

        #[tokio::test]
        async fn post_with_json() -> anyhow::Result<()> {
            let server = MockServer::start_async().await;
            let (mock, client) = setup_with_fake_api_key(&server).await;

            let _: serde_json::Value = client
                .clone()
                .gateway_request()
                .with_method("")
                .with_retry(false)
                .post_with_json(&json!({}))
                .await?;

            let _: serde_json::Value = client
                .clone()
                .feeder_gateway_request()
                .with_method("")
                .with_retry(false)
                .post_with_json(&json!({}))
                .await?;

            mock.assert_hits(2);

            Ok(())
        }
    }
}
