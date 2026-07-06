use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) struct Ctx {
    pub client: reqwest::Client,
    pub url:    String,
}

pub(super) trait HttpMethod: Send + 'static {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder;
}

pub(super) struct Get;
pub(super) struct Post;
pub(super) struct Put;
pub(super) struct Delete;

impl HttpMethod for Get    { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.get(u)    } }
impl HttpMethod for Post   { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.post(u)   } }
impl HttpMethod for Put    { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.put(u)    } }
impl HttpMethod for Delete { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.delete(u) } }

pub(super) trait HttpBody: Send + 'static {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder;
}

pub(super) struct NoBody;

// Pre built body bytes + content-type header value.
pub(super) struct JsonBody {
    bytes: bytes::Bytes,
}

impl JsonBody {
    pub fn new(json: String) -> Self {
        Self { bytes: bytes::Bytes::from(json.into_bytes()) }
    }
}

impl HttpBody for NoBody {
    #[inline(always)]
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder { req }
}

impl HttpBody for JsonBody {
    #[inline(always)]
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header(reqwest::header::CONTENT_TYPE, "application/json")
           .body(self.bytes.clone()) // clone is O(1) on Bytes
    }
}

// A single request outcome.  Two u64s = 16 bytes, one cache line word
#[derive(Copy, Clone)]
pub(super) struct Sample {
    // Latency in nanoseconds, 0 means failure
    pub latency_ns: u64,
    // Always 1 (carried for lock free accumulation without an extra atomic)
    pub sent: u64,
}

impl Sample {
    #[inline(always)]
    pub fn ok(ns: u64) -> Self { Self { latency_ns: ns, sent: 1 } }
    #[inline(always)]
    pub fn fail() -> Self { Self { latency_ns: 0, sent: 1 } }
    #[inline(always)]
    pub fn is_ok(self) -> bool { self.latency_ns > 0 }
}

// One async task, pull work tokens, fire requests, push results
pub(super) async fn run<M: HttpMethod, B: HttpBody>(
    token:   CancellationToken,
    work_rx: flume::Receiver<()>,
    ctx:     Arc<Ctx>,
    tx:      flume::Sender<Sample>,
    body:    B,
) {
    loop {
        // Check cancellation first
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            job = work_rx.recv_async() => {
                match job {
                    Err(_) => break, // producer dropped → benchmark is over
                    Ok(()) => {
                        let t0  = std::time::Instant::now();
                        let req = body.apply(M::build(&ctx.client, &ctx.url));

                        let sample = match req.send().await {
                            Ok(resp) if resp.status().is_success() =>
                                Sample::ok(t0.elapsed().as_nanos() as u64),
                            _ =>
                                Sample::fail(),
                        };

                        if tx.send_async(sample).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}