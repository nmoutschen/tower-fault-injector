//! # Latency injection for `tower`
//!
//! Layer that injects a random amount of latency into a service.
//!
//! ## Usage
//!
//! ```rust
//! use tower_fault_injector::latency::LatencyLayer;
//! use tower::{service_fn, ServiceBuilder};
//! # async fn my_service() -> Result<(), ()> {
//! #     Ok(())
//! # }
//!
//! // Initialize a LatencyLayer with a 10% probability of injecting
//! // 200 to 500 milliseconds of latency.
//! let latency_layer = LatencyLayer::new(0.1, 200..500).unwrap();
//!
//! let service = ServiceBuilder::new()
//!     .layer(latency_layer)
//!     .service(service_fn(my_service));
//! ```

use rand::{
    distributions::{Bernoulli, BernoulliError},
    prelude::*,
};
use std::{
    future::Future,
    marker::PhantomData,
    ops::Range,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time;
use tower::{Layer, Service};

/// A layer that adds latency to the service before sending a request.
///
/// This adds a random amount of latency to a random percentage of requests.
#[derive(Debug, Clone)]
pub struct LatencyLayer<'a> {
    distribution: Bernoulli,
    range: Range<u64>,
    _phantom: PhantomData<&'a ()>,
}

impl<'a> LatencyLayer<'a> {
    /// Create a new `LatencyLayer` with the given probability and latency range.
    ///
    /// The probability is the chance that a request will be delayed, bound
    /// between 0 and 1. A probability of 0.5 means that 50% of the calls
    /// to the service will result in elevated latencies.
    ///
    /// The range is the range of latency to add, in milliseconds.
    pub fn new(probability: f64, range: Range<u64>) -> Result<Self, Error> {
        Ok(LatencyLayer {
            distribution: Bernoulli::new(probability)?,
            range,
            _phantom: PhantomData,
        })
    }
}

impl<'a> Default for LatencyLayer<'a> {
    fn default() -> Self {
        LatencyLayer::new(0.1, 100..200).expect("failed to create default latency layer")
    }
}

impl<'a, S> Layer<S> for LatencyLayer<'a> {
    type Service = LatencyService<'a, S>;

    fn layer(&self, inner: S) -> Self::Service {
        LatencyService {
            inner,
            layer: self.clone(),
            rng: StdRng::from_entropy(),
        }
    }
}

/// Underlying service for the `LatencyLayer`
#[derive(Debug, Clone)]
pub struct LatencyService<'a, S> {
    inner: S,
    layer: LatencyLayer<'a>,
    rng: StdRng,
}

impl<'a, R, S> Service<R> for LatencyService<'a, S>
where
    R: Send,
    S: Service<R> + Send,
    S::Future: Send + 'a,
    S::Response: Send,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = LatencyFuture<'a, R, S>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: R) -> Self::Future {
        // Calculate latency
        let latency = if self.layer.distribution.sample(&mut self.rng) {
            self.rng.gen_range(self.layer.range.clone())
        } else {
            0
        };

        let fut = self.inner.call(request);
        let fut = async move {
            time::sleep(Duration::from_millis(latency)).await;
            fut.await
        };

        Box::pin(fut)
    }
}

type LatencyFuture<'a, R, S> = Pin<
    Box<
        dyn Future<Output = Result<<S as Service<R>>::Response, <S as Service<R>>::Error>>
            + Send
            + 'a,
    >,
>;

/// Errors that can be returned by the `LatencyLayer`.
#[derive(Debug)]
pub enum Error {
    /// Error creating an `LatencyLayer`
    NewLayerError(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NewLayerError(s) => write!(f, "cannot create the layer: {}", s),
        }
    }
}

impl From<BernoulliError> for Error {
    fn from(_err: BernoulliError) -> Self {
        Error::NewLayerError("invalid probability")
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use std::time::Instant;

    #[tokio::test]
    async fn latency_none() -> Result<(), Error> {
        let latency = LatencyLayer::new(0.0, 10..20)?;
        let mut service = latency.layer(DummyService);

        for _ in 0..1000 {
            let now = Instant::now();
            let _res = service.call(()).await;
            let elapsed = now.elapsed();

            assert!(elapsed < Duration::from_millis(5));
        }

        Ok(())
    }

    #[tokio::test]
    async fn latency_all() -> Result<(), Error> {
        let latency = LatencyLayer::new(1.0, 10..11)?;
        let mut service = latency.layer(DummyService);

        for _ in 0..100 {
            let now = Instant::now();
            let _res = service.call(()).await;
            let elapsed = now.elapsed();

            assert!(elapsed > Duration::from_millis(5));
        }

        Ok(())
    }
}
