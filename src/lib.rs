//! # Tracing OpenTelemetry
//!
//! [`tracing`] is a framework for instrumenting Rust programs to collect
//! structured, event-based diagnostic information. This crate provides a layer
//! that connects spans from multiple systems into a trace and emits them to
//! [OpenTelemetry]-compatible distributed tracing systems for processing and
//! visualization.
//!
//! [OpenTelemetry]: https://opentelemetry.io
//! [`tracing`]: https://github.com/tokio-rs/tracing
//!
//! ### Special Fields
//!
//! Fields with an `otel.` prefix are reserved for this crate and have specific
//! meaning. They are treated as ordinary fields by other layers. The current
//! special fields are:
//!
//! * `otel.name`: Override the span name sent to OpenTelemetry exporters.
//!   Setting this field is useful if you want to display non-static information
//!   in your span name.
//! * `otel.kind`: Set the span kind to one of the supported OpenTelemetry [span kinds]. These must
//!   be specified as strings such as `"client"` or `"server"`. If it is not specified, the span is
//!   assumed to be internal.
//! * `otel.status_code`: Set the span status code to one of the supported OpenTelemetry [span status codes].
//! * `otel.status_description`: Set the span description of the status. This should be used only if
//!   `otel.status_code` is also set.
//!
//! [span kinds]: opentelemetry::trace::SpanKind
//! [span status codes]: opentelemetry::trace::Status
//!
//! ### Semantic Conventions
//!
//! OpenTelemetry defines conventional names for attributes of common
//! operations. These names can be assigned directly as fields, e.g.
//! `trace_span!("request", "server.port" = 80, "url.full" = ..)`, and they
//! will be passed through to your configured OpenTelemetry exporter. You can
//! find the full list of the operations and their expected field names in the
//! [semantic conventions] spec.
//!
//! [semantic conventions]: https://github.com/open-telemetry/semantic-conventions
//!
//! ### Stability Status
//!
//! The OpenTelemetry tracing specification is stable but the underlying [opentelemetry crate] is
//! not so some breaking changes will still occur in this crate as well. Metrics are not yet fully
//! stable. You can read the specification via the [spec repository].
//!
//! [opentelemetry crate]: https://github.com/open-telemetry/opentelemetry-rust
//! [spec repository]: https://github.com/open-telemetry/opentelemetry-specification
//!
//! ### OpenTelemetry Logging
//!
//! Logging to OpenTelemetry collectors is not supported by this crate, only traces and metrics are.
//! If you need to export logs through OpenTelemetry, consider [`opentelemetry-appender-tracing`].
//!
//! [`opentelemetry-appender-tracing`]: https://crates.io/crates/opentelemetry-appender-tracing
//!
//! ## Examples
//!
//! ```
//! use opentelemetry_sdk::trace::SdkTracerProvider;
//! use opentelemetry::trace::{Tracer, TracerProvider as _};
//! use tracing::{error, span};
//! use tracing_subscriber::layer::SubscriberExt;
//! use tracing_subscriber::Registry;
//!
//! // Create a new OpenTelemetry trace pipeline that prints to stdout
//! let provider = SdkTracerProvider::builder()
//!     .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
//!     .build();
//! let tracer = provider.tracer("readme_example");
//!
//! // Create a tracing layer with the configured tracer
//! let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
//!
//! // Use the tracing subscriber `Registry`, or any other subscriber
//! // that impls `LookupSpan`
//! let subscriber = Registry::default().with(telemetry);
//!
//! // Trace executed code
//! tracing::subscriber::with_default(subscriber, || {
//!     // Spans will be sent to the configured OpenTelemetry exporter
//!     let root = span!(tracing::Level::TRACE, "app_start", work_units = 2);
//!     let _enter = root.enter();
//!
//!     error!("This event will be logged in the root span.");
//! });
//! ```
//!
//! ## Feature Flags
//!
//! - `metrics`: Enables the [`MetricsLayer`] type, a [layer] that
//!   exports OpenTelemetry metrics from specifically-named events. This enables
//!   the `metrics` feature flag on the `opentelemetry` crate.  *Enabled by
//!   default*.
//!
//! [layer]: tracing_subscriber::layer
#![warn(unreachable_pub)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/tokio-rs/tracing/master/assets/logo-type.png"
)]
#![cfg_attr(
    docsrs,
    // Allows displaying cfgs/feature flags in the documentation.
    feature(doc_cfg, doc_auto_cfg),
    // Allows adding traits to RustDoc's list of "notable traits"
    feature(doc_notable_trait),
    // Fail the docs build if any intra-docs links are broken
    deny(rustdoc::broken_intra_doc_links),
)]

/// Implementation of the trace::Subscriber trait; publishes OpenTelemetry metrics.
#[cfg(feature = "metrics")]
mod metrics;

/// Implementation of the trace::Layer as a source of OpenTelemetry data.
mod layer;
/// Span extension which enables OpenTelemetry context management.
mod span_ext;
/// Protocols for OpenTelemetry Tracers that are compatible with Tracing
mod tracer;

pub use layer::{layer, OpenTelemetryLayer};

#[cfg(feature = "metrics")]
pub use metrics::MetricsLayer;
pub use span_ext::OpenTelemetrySpanExt;
pub use tracer::PreSampledTracer;

/// Per-span OpenTelemetry data tracked by this crate.
///
/// Useful for implementing [PreSampledTracer] in alternate otel SDKs.
#[derive(Debug, Clone)]
pub struct OtelData {
    /// The parent otel `Context` for the current tracing span.
    pub parent_cx: opentelemetry::Context,

    /// The otel span data recorded during the current tracing span.
    pub builder: opentelemetry::trace::SpanBuilder,
}

pub(crate) mod time {
    use std::time::SystemTime;

    #[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"))))]
    pub(crate) fn now() -> SystemTime {
        SystemTime::now()
    }

    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
    pub(crate) fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(js_sys::Date::now() as u64)
    }
}
