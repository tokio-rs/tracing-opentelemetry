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
    feature(doc_cfg),
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
/// Function which enables OpenTelemetry context extraction from span extensions.
mod otel_context;
/// Span extension which enables OpenTelemetry context management.
mod span_ext;

mod stack;

use std::time::SystemTime;

pub use layer::{layer, FilteredOpenTelemetryLayer, OpenTelemetryLayer};

#[cfg(feature = "metrics")]
pub use metrics::MetricsLayer;
use opentelemetry::trace::TraceContextExt as _;
pub use otel_context::get_otel_context;
pub use span_ext::{OpenTelemetrySpanExt, SetParentError};

/// Per-span OpenTelemetry data tracked by this crate.
#[derive(Debug)]
pub struct OtelData {
    /// The state of the OtelData, which can either be a builder or a context.
    state: OtelDataState,
    /// The end time of the span if it has been exited.
    end_time: Option<SystemTime>,
}

impl OtelData {
    /// Gets the trace ID of the span.
    ///
    /// Returns `None` if the context has not been built yet. This can be forced e.g. by calling
    /// [`context`] on the span (not on `OtelData`) or if [context activation] was not explicitly
    /// opted-out of, simply entering the span for the first time.
    ///
    /// [`context`]: OpenTelemetrySpanExt::context
    /// [context activation]: OpenTelemetryLayer::with_context_activation
    pub fn trace_id(&self) -> Option<opentelemetry::TraceId> {
        if let OtelDataState::Context { current_cx } = &self.state {
            Some(current_cx.span().span_context().trace_id())
        } else {
            None
        }
    }

    /// Gets the span ID of the span.
    ///
    /// Returns `None` if the context has not been built yet. This can be forced e.g. by calling
    /// [`context`] on the span (not on `OtelData`) or if [context activation] was not explicitly
    /// opted-out of, simply entering the span for the first time.
    ///
    /// [`context`]: OpenTelemetrySpanExt::context
    /// [context activation]: OpenTelemetryLayer::with_context_activation
    pub fn span_id(&self) -> Option<opentelemetry::SpanId> {
        if let OtelDataState::Context { current_cx } = &self.state {
            Some(current_cx.span().span_context().span_id())
        } else {
            None
        }
    }
}

/// The state of the OpenTelemetry data for a span.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum OtelDataState {
    /// The span is being built, with a parent context and a builder.
    Builder {
        parent_cx: opentelemetry::Context,
        builder: opentelemetry::trace::SpanBuilder,
        status: opentelemetry::trace::Status,
    },
    /// The span has been started or accessed and is now in a context.
    Context { current_cx: opentelemetry::Context },
}

impl Default for OtelDataState {
    fn default() -> Self {
        OtelDataState::Context {
            current_cx: opentelemetry::Context::default(),
        }
    }
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
