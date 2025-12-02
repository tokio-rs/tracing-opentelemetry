use crate::stack::IdValueStack;
use crate::{OtelData, OtelDataState};
pub use filtered::FilteredOpenTelemetryLayer;
use opentelemetry::ContextGuard;
use opentelemetry::{
    trace::{self as otel, noop, Span, SpanBuilder, SpanKind, Status, TraceContextExt},
    Context as OtelContext, Key, KeyValue, StringValue, Value,
};
use std::cell::RefCell;
use std::thread;
#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"))))]
use std::time::Instant;
use std::{any::TypeId, borrow::Cow};
use std::{fmt, vec};
use std::{marker, mem::take};
use tracing_core::span::{self, Attributes, Id, Record};
use tracing_core::{field, Event, Subscriber};
#[cfg(feature = "tracing-log")]
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::Filter;
use tracing_subscriber::registry::{ExtensionsMut, LookupSpan};
use tracing_subscriber::Layer;
#[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
use web_time::Instant;

mod filtered;

const SPAN_NAME_FIELD: &str = "otel.name";
const SPAN_KIND_FIELD: &str = "otel.kind";
const SPAN_STATUS_CODE_FIELD: &str = "otel.status_code";
const SPAN_STATUS_DESCRIPTION_FIELD: &str = "otel.status_description";
const SPAN_EVENT_COUNT_FIELD: &str = "otel.tracing_event_count";

const EVENT_EXCEPTION_NAME: &str = "exception";
const FIELD_EXCEPTION_MESSAGE: &str = "exception.message";
const FIELD_EXCEPTION_STACKTRACE: &str = "exception.stacktrace";

/// An [OpenTelemetry] propagation layer for use in a project that uses
/// [tracing].
///
/// [OpenTelemetry]: https://opentelemetry.io
/// [tracing]: https://github.com/tokio-rs/tracing
pub struct OpenTelemetryLayer<S, T> {
    tracer: T,
    location: bool,
    tracked_inactivity: bool,
    with_threads: bool,
    with_level: bool,
    with_target: bool,
    context_activation: bool,
    sem_conv_config: SemConvConfig,
    with_context: WithContext,
    _registry: marker::PhantomData<S>,
}

impl<S> Default for OpenTelemetryLayer<S, noop::NoopTracer>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn default() -> Self {
        OpenTelemetryLayer::new(noop::NoopTracer::new())
    }
}

/// Construct a layer to track spans via [OpenTelemetry].
///
/// It will convert the tracing spans to OpenTelemetry spans
/// and tracing events which are in a span to events in the currently active OpenTelemetry span.
/// Child-Parent links will be automatically translated as well.
/// Follows-From links will also be generated, but only if the originating span has not been closed yet.
///
/// Various translations exist between semantic conventions of OpenTelemetry and `tracing`,
/// such as including level information as fields, or mapping error fields to exceptions.
/// See the various `layer().with_*` functions for configuration of these features.
///
/// [OpenTelemetry]: https://opentelemetry.io
///
/// # Examples
///
/// ```rust,no_run
/// use tracing_subscriber::layer::SubscriberExt;
/// use tracing_subscriber::Registry;
///
/// // Use the tracing subscriber `Registry`, or any other subscriber
/// // that impls `LookupSpan`
/// let subscriber = Registry::default().with(tracing_opentelemetry::layer());
/// # drop(subscriber);
/// ```
pub fn layer<S>() -> OpenTelemetryLayer<S, noop::NoopTracer>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    OpenTelemetryLayer::default()
}

///
/// This struct lets us call back into the layer from the [crate::OpenTelemetrySpanExt] methods,
/// letting us access and mutate the underlying data on the layer side in the context of
/// tokio-tracing's span operations.
///
/// The functions on this struct "remember" the types of the subscriber so that we
/// can downcast to something aware of them without knowing those
/// types at the callsite.
///
/// See https://github.com/tokio-rs/tracing/blob/4dad420ee1d4607bad79270c1520673fa6266a3d/tracing-error/src/layer.rs
pub(crate) struct WithContext {
    ///
    /// Provides access to the OtelData associated with the given span ID.
    ///
    #[allow(clippy::type_complexity)]
    pub(crate) with_context: fn(&tracing::Dispatch, &span::Id, f: &mut dyn FnMut(&mut OtelData)),

    ///
    /// Ensures the given SpanId has been activated - that is, created in the OTel side of things,
    /// and had its SpanBuilder consumed - and then provides access to the OtelData associated with it.
    ///
    #[allow(clippy::type_complexity)]
    pub(crate) with_activated_context:
        fn(&tracing::Dispatch, &span::Id, f: &mut dyn FnMut(&mut OtelData)),

    ///
    /// Ensures the given SpanId has been activated - that is, created in the OTel side of things,
    /// and had its SpanBuilder consumed - and then provides access to the OTel Context associated with it.
    ///
    #[allow(clippy::type_complexity)]
    pub(crate) with_activated_otel_context:
        fn(&tracing::Dispatch, &mut ExtensionsMut<'_>, f: &mut dyn FnMut(&OtelContext)),
}

impl WithContext {
    ///
    /// Return the OtelData associated with the given spanId.
    ///
    pub(crate) fn with_context(
        &self,
        dispatch: &tracing::Dispatch,
        id: &span::Id,
        mut f: impl FnMut(&mut OtelData),
    ) {
        (self.with_context)(dispatch, id, &mut f)
    }

    ///
    /// If the span associated with the given SpanId has not yet been
    /// built, build it, consuming the span ID.
    ///
    /// Optionally performs additional operations on the OtelData after building.
    ///
    pub(crate) fn with_activated_context(
        &self,
        dispatch: &tracing::Dispatch,
        id: &span::Id,
        mut f: impl FnMut(&mut OtelData),
    ) {
        (self.with_activated_context)(dispatch, id, &mut f)
    }

    ///
    /// Ensures the given SpanId has been activated - that is, created in the OTel side of things,
    /// and had its SpanBuilder consumed - and then provides access to the OtelData associated with it.
    ///
    #[allow(clippy::type_complexity)]
    pub(crate) fn with_activated_otel_context(
        &self,
        dispatch: &tracing::Dispatch,
        extensions: &mut ExtensionsMut<'_>,
        mut f: impl FnMut(&OtelContext),
    ) {
        (self.with_activated_otel_context)(dispatch, extensions, &mut f)
    }
}

fn str_to_span_kind(s: &str) -> Option<otel::SpanKind> {
    match s {
        s if s.eq_ignore_ascii_case("server") => Some(otel::SpanKind::Server),
        s if s.eq_ignore_ascii_case("client") => Some(otel::SpanKind::Client),
        s if s.eq_ignore_ascii_case("producer") => Some(otel::SpanKind::Producer),
        s if s.eq_ignore_ascii_case("consumer") => Some(otel::SpanKind::Consumer),
        s if s.eq_ignore_ascii_case("internal") => Some(otel::SpanKind::Internal),
        _ => None,
    }
}

fn str_to_status(s: &str) -> otel::Status {
    match s {
        s if s.eq_ignore_ascii_case("ok") => otel::Status::Ok,
        s if s.eq_ignore_ascii_case("error") => otel::Status::error(""),
        _ => otel::Status::Unset,
    }
}

#[derive(Default)]
struct SpanBuilderUpdates {
    name: Option<Cow<'static, str>>,
    span_kind: Option<SpanKind>,
    status: Option<Status>,
    attributes: Option<Vec<KeyValue>>,
}

impl SpanBuilderUpdates {
    fn update(self, span_builder: &mut SpanBuilder, s: &mut Status) {
        let Self {
            name,
            span_kind,
            status,
            attributes,
        } = self;

        if let Some(name) = name {
            span_builder.name = name;
        }
        if let Some(span_kind) = span_kind {
            span_builder.span_kind = Some(span_kind);
        }
        if let Some(status) = status {
            *s = status;
        }
        if let Some(attributes) = attributes {
            if let Some(builder_attributes) = &mut span_builder.attributes {
                builder_attributes.extend(attributes);
            } else {
                span_builder.attributes = Some(attributes);
            }
        }
    }

    fn update_span(self, span: &opentelemetry::trace::SpanRef<'_>) {
        let Self {
            status, attributes, ..
        } = self;

        if let Some(status) = status {
            span.set_status(status);
        }
        if let Some(attributes) = attributes {
            span.set_attributes(attributes);
        }
    }
}

struct SpanEventVisitor<'a, 'b> {
    event_builder: &'a mut otel::Event,
    span_builder_updates: &'b mut Option<SpanBuilderUpdates>,
    sem_conv_config: SemConvConfig,
}

impl field::Visit for SpanEventVisitor<'_, '_> {
    /// Record events on the underlying OpenTelemetry [`Span`] from `bool` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_bool(&mut self, field: &field::Field, value: bool) {
        match field.name() {
            "message" => self.event_builder.name = value.to_string().into(),
            // Skip fields that are actually log metadata that have already been handled
            #[cfg(feature = "tracing-log")]
            name if name.starts_with("log.") => (),
            name => {
                self.event_builder
                    .attributes
                    .push(KeyValue::new(name, value));
            }
        }
    }

    /// Record events on the underlying OpenTelemetry [`Span`] from `f64` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_f64(&mut self, field: &field::Field, value: f64) {
        match field.name() {
            "message" => self.event_builder.name = value.to_string().into(),
            // Skip fields that are actually log metadata that have already been handled
            #[cfg(feature = "tracing-log")]
            name if name.starts_with("log.") => (),
            name => {
                self.event_builder
                    .attributes
                    .push(KeyValue::new(name, value));
            }
        }
    }

    /// Record events on the underlying OpenTelemetry [`Span`] from `i64` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_i64(&mut self, field: &field::Field, value: i64) {
        match field.name() {
            "message" => self.event_builder.name = value.to_string().into(),
            // Skip fields that are actually log metadata that have already been handled
            #[cfg(feature = "tracing-log")]
            name if name.starts_with("log.") => (),
            name => {
                self.event_builder
                    .attributes
                    .push(KeyValue::new(name, value));
            }
        }
    }

    /// Record events on the underlying OpenTelemetry [`Span`] from `&str` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_str(&mut self, field: &field::Field, value: &str) {
        match field.name() {
            "message" => self.event_builder.name = value.to_string().into(),
            // While tracing supports the error primitive, the instrumentation macro does not
            // use the primitive and instead uses the debug or display primitive.
            // In both cases, an event with an empty name and with an error attribute is created.
            "error" if self.event_builder.name.is_empty() => {
                if self.sem_conv_config.error_events_to_status {
                    self.span_builder_updates
                        .get_or_insert_with(SpanBuilderUpdates::default)
                        .status
                        .replace(otel::Status::error(format!("{value:?}")));
                }
                if self.sem_conv_config.error_events_to_exceptions {
                    self.event_builder.name = EVENT_EXCEPTION_NAME.into();
                    self.event_builder
                        .attributes
                        .push(KeyValue::new(FIELD_EXCEPTION_MESSAGE, format!("{value:?}")));
                } else {
                    self.event_builder
                        .attributes
                        .push(KeyValue::new("error", format!("{value:?}")));
                }
            }
            // Skip fields that are actually log metadata that have already been handled
            #[cfg(feature = "tracing-log")]
            name if name.starts_with("log.") => (),
            name => {
                self.event_builder
                    .attributes
                    .push(KeyValue::new(name, value.to_string()));
            }
        }
    }

    /// Record events on the underlying OpenTelemetry [`Span`] from values that
    /// implement Debug.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_debug(&mut self, field: &field::Field, value: &dyn fmt::Debug) {
        match field.name() {
            "message" => self.event_builder.name = format!("{value:?}").into(),
            // While tracing supports the error primitive, the instrumentation macro does not
            // use the primitive and instead uses the debug or display primitive.
            // In both cases, an event with an empty name and with an error attribute is created.
            "error" if self.event_builder.name.is_empty() => {
                if self.sem_conv_config.error_events_to_status {
                    self.span_builder_updates
                        .get_or_insert_with(SpanBuilderUpdates::default)
                        .status
                        .replace(otel::Status::error(format!("{value:?}")));
                }
                if self.sem_conv_config.error_events_to_exceptions {
                    self.event_builder.name = EVENT_EXCEPTION_NAME.into();
                    self.event_builder
                        .attributes
                        .push(KeyValue::new(FIELD_EXCEPTION_MESSAGE, format!("{value:?}")));
                } else {
                    self.event_builder
                        .attributes
                        .push(KeyValue::new("error", format!("{value:?}")));
                }
            }
            // Skip fields that are actually log metadata that have already been handled
            #[cfg(feature = "tracing-log")]
            name if name.starts_with("log.") => (),
            name => {
                self.event_builder
                    .attributes
                    .push(KeyValue::new(name, format!("{value:?}")));
            }
        }
    }

    /// Set attributes on the underlying OpenTelemetry [`Span`] using a [`std::error::Error`]'s
    /// [`std::fmt::Display`] implementation. Also adds the `source` chain as an extra field
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_error(
        &mut self,
        field: &tracing_core::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        let mut chain: Vec<StringValue> = Vec::new();
        let mut next_err = value.source();

        while let Some(err) = next_err {
            chain.push(err.to_string().into());
            next_err = err.source();
        }

        let error_msg = value.to_string();

        if self.sem_conv_config.error_fields_to_exceptions {
            self.event_builder.attributes.push(KeyValue::new(
                Key::new(FIELD_EXCEPTION_MESSAGE),
                Value::String(StringValue::from(error_msg.clone())),
            ));

            // NOTE: This is actually not the stacktrace of the exception. This is
            // the "source chain". It represents the heirarchy of errors from the
            // app level to the lowest level such as IO. It does not represent all
            // of the callsites in the code that led to the error happening.
            // `std::error::Error::backtrace` is a nightly-only API and cannot be
            // used here until the feature is stabilized.
            self.event_builder.attributes.push(KeyValue::new(
                Key::new(FIELD_EXCEPTION_STACKTRACE),
                Value::Array(chain.clone().into()),
            ));
        }

        if self.sem_conv_config.error_records_to_exceptions {
            let attributes = self
                .span_builder_updates
                .get_or_insert_with(SpanBuilderUpdates::default)
                .attributes
                .get_or_insert_with(Vec::new);

            attributes.push(KeyValue::new(
                FIELD_EXCEPTION_MESSAGE,
                Value::String(error_msg.clone().into()),
            ));

            // NOTE: This is actually not the stacktrace of the exception. This is
            // the "source chain". It represents the heirarchy of errors from the
            // app level to the lowest level such as IO. It does not represent all
            // of the callsites in the code that led to the error happening.
            // `std::error::Error::backtrace` is a nightly-only API and cannot be
            // used here until the feature is stabilized.
            attributes.push(KeyValue::new(
                FIELD_EXCEPTION_STACKTRACE,
                Value::Array(chain.clone().into()),
            ));
        }

        self.event_builder.attributes.push(KeyValue::new(
            Key::new(field.name()),
            Value::String(StringValue::from(error_msg)),
        ));
        self.event_builder.attributes.push(KeyValue::new(
            Key::new(format!("{}.chain", field.name())),
            Value::Array(chain.into()),
        ));
    }
}

/// Control over the mapping between tracing fields/events and OpenTelemetry conventional status/exception fields
#[derive(Clone, Copy)]
struct SemConvConfig {
    /// If an error value is recorded on an event/span, should the otel fields
    /// be added
    ///
    /// Note that this uses tracings `record_error` which is only implemented for `(dyn Error + 'static)`.
    error_fields_to_exceptions: bool,

    /// If an error value is recorded on an event, should the otel fields be
    /// added to the corresponding span
    ///
    /// Note that this uses tracings `record_error` which is only implemented for `(dyn Error + 'static)`.
    error_records_to_exceptions: bool,

    /// If a function is instrumented and returns a `Result`, should the error
    /// value be propagated to the span status.
    ///
    /// Without this enabled, the span status will be "Error" with an empty description
    /// when at least one error event is recorded in the span.
    ///
    /// Note: the instrument macro will emit an error event if the function returns the `Err` variant.
    /// This is not affected by this setting. Disabling this will only affect the span status.
    error_events_to_status: bool,

    /// If an event with an empty name and a field named `error` is recorded,
    /// should the event be rewritten to have the name `exception` and the field `exception.message`
    ///
    /// Follows the semantic conventions for exceptions.
    ///
    /// Note: the instrument macro will emit an error event if the function returns the `Err` variant.
    /// This is not affected by this setting. Disabling this will only affect the created fields on the OTel span.
    error_events_to_exceptions: bool,
}

struct SpanAttributeVisitor<'a> {
    span_builder_updates: &'a mut SpanBuilderUpdates,
    sem_conv_config: SemConvConfig,
}

impl SpanAttributeVisitor<'_> {
    fn record(&mut self, attribute: KeyValue) {
        self.span_builder_updates
            .attributes
            .get_or_insert_with(Vec::new)
            .push(KeyValue::new(attribute.key, attribute.value));
    }
}

impl field::Visit for SpanAttributeVisitor<'_> {
    /// Set attributes on the underlying OpenTelemetry [`Span`] from `bool` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_bool(&mut self, field: &field::Field, value: bool) {
        self.record(KeyValue::new(field.name(), value));
    }

    /// Set attributes on the underlying OpenTelemetry [`Span`] from `f64` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_f64(&mut self, field: &field::Field, value: f64) {
        self.record(KeyValue::new(field.name(), value));
    }

    /// Set attributes on the underlying OpenTelemetry [`Span`] from `i64` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_i64(&mut self, field: &field::Field, value: i64) {
        self.record(KeyValue::new(field.name(), value));
    }

    /// Set attributes on the underlying OpenTelemetry [`Span`] from `&str` values.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_str(&mut self, field: &field::Field, value: &str) {
        match field.name() {
            SPAN_NAME_FIELD => self.span_builder_updates.name = Some(value.to_string().into()),
            SPAN_KIND_FIELD => self.span_builder_updates.span_kind = str_to_span_kind(value),
            SPAN_STATUS_CODE_FIELD => self.span_builder_updates.status = Some(str_to_status(value)),
            SPAN_STATUS_DESCRIPTION_FIELD => {
                self.span_builder_updates.status = Some(otel::Status::error(value.to_string()))
            }
            _ => self.record(KeyValue::new(field.name(), value.to_string())),
        }
    }

    /// Set attributes on the underlying OpenTelemetry [`Span`] from values that
    /// implement Debug.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_debug(&mut self, field: &field::Field, value: &dyn fmt::Debug) {
        match field.name() {
            SPAN_NAME_FIELD => self.span_builder_updates.name = Some(format!("{value:?}").into()),
            SPAN_KIND_FIELD => {
                self.span_builder_updates.span_kind = str_to_span_kind(&format!("{value:?}"))
            }
            SPAN_STATUS_CODE_FIELD => {
                self.span_builder_updates.status = Some(str_to_status(&format!("{value:?}")))
            }
            SPAN_STATUS_DESCRIPTION_FIELD => {
                self.span_builder_updates.status = Some(otel::Status::error(format!("{value:?}")))
            }
            _ => self.record(KeyValue::new(
                Key::new(field.name()),
                Value::String(format!("{value:?}").into()),
            )),
        }
    }

    /// Set attributes on the underlying OpenTelemetry [`Span`] using a [`std::error::Error`]'s
    /// [`std::fmt::Display`] implementation. Also adds the `source` chain as an extra field
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn record_error(
        &mut self,
        field: &tracing_core::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        let mut chain: Vec<StringValue> = Vec::new();
        let mut next_err = value.source();

        while let Some(err) = next_err {
            chain.push(err.to_string().into());
            next_err = err.source();
        }

        let error_msg = value.to_string();

        if self.sem_conv_config.error_fields_to_exceptions {
            self.record(KeyValue::new(
                Key::new(FIELD_EXCEPTION_MESSAGE),
                Value::from(error_msg.clone()),
            ));

            // NOTE: This is actually not the stacktrace of the exception. This is
            // the "source chain". It represents the heirarchy of errors from the
            // app level to the lowest level such as IO. It does not represent all
            // of the callsites in the code that led to the error happening.
            // `std::error::Error::backtrace` is a nightly-only API and cannot be
            // used here until the feature is stabilized.
            self.record(KeyValue::new(
                Key::new(FIELD_EXCEPTION_STACKTRACE),
                Value::Array(chain.clone().into()),
            ));
        }

        self.record(KeyValue::new(
            Key::new(field.name()),
            Value::String(error_msg.into()),
        ));
        self.record(KeyValue::new(
            Key::new(format!("{}.chain", field.name())),
            Value::Array(chain.into()),
        ));
    }
}

impl<S, T> OpenTelemetryLayer<S, T>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    T: otel::Tracer + 'static,
    T::Span: Send + Sync,
{
    /// Set the [`Tracer`] that this layer will use to produce and track
    /// OpenTelemetry [`Span`]s.
    ///
    /// [`Tracer`]: opentelemetry::trace::Tracer
    /// [`Span`]: opentelemetry::trace::Span
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tracing_opentelemetry::OpenTelemetryLayer;
    /// use tracing_subscriber::layer::SubscriberExt;
    /// use opentelemetry::trace::TracerProvider as _;
    /// use tracing_subscriber::Registry;
    ///
    /// // Create an OTLP pipeline exporter for a `trace_demo` service.
    ///
    /// let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
    ///     .with_tonic()
    ///     .build()
    ///     .unwrap();
    ///
    /// let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
    ///     .with_simple_exporter(otlp_exporter)
    ///     .build()
    ///     .tracer("trace_demo");
    ///
    /// // Create a layer with the configured tracer
    /// let otel_layer = OpenTelemetryLayer::new(tracer);
    ///
    /// // Use the tracing subscriber `Registry`, or any other subscriber
    /// // that impls `LookupSpan`
    /// let subscriber = Registry::default().with(otel_layer);
    /// # drop(subscriber);
    /// ```
    pub fn new(tracer: T) -> Self {
        OpenTelemetryLayer {
            tracer,
            location: true,
            tracked_inactivity: true,
            with_threads: true,
            with_level: false,
            with_target: true,
            context_activation: true,
            sem_conv_config: SemConvConfig {
                error_fields_to_exceptions: true,
                error_records_to_exceptions: true,
                error_events_to_exceptions: true,
                error_events_to_status: true,
            },
            with_context: WithContext {
                with_context: Self::get_context,
                with_activated_context: Self::get_activated_context,
                with_activated_otel_context: Self::get_activated_otel_context,
            },
            _registry: marker::PhantomData,
        }
    }

    /// Set the [`Tracer`] that this layer will use to produce and track
    /// OpenTelemetry [`Span`]s.
    ///
    /// [`Tracer`]: opentelemetry::trace::Tracer
    /// [`Span`]: opentelemetry::trace::Span
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tracing_subscriber::layer::SubscriberExt;
    /// use tracing_subscriber::Registry;
    /// use opentelemetry::trace::TracerProvider;
    ///
    /// // Create an OTLP pipeline exporter for a `trace_demo` service.
    ///
    /// let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
    ///     .with_tonic()
    ///     .build()
    ///     .unwrap();
    ///
    /// let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
    ///     .with_simple_exporter(otlp_exporter)
    ///     .build()
    ///     .tracer("trace_demo");
    ///
    /// // Create a layer with the configured tracer
    /// let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    ///
    /// // Use the tracing subscriber `Registry`, or any other subscriber
    /// // that impls `LookupSpan`
    /// let subscriber = Registry::default().with(otel_layer);
    /// # drop(subscriber);
    /// ```
    pub fn with_tracer<Tracer>(self, tracer: Tracer) -> OpenTelemetryLayer<S, Tracer>
    where
        Tracer: otel::Tracer + 'static,
        Tracer::Span: Send + Sync,
    {
        OpenTelemetryLayer {
            tracer,
            location: self.location,
            tracked_inactivity: self.tracked_inactivity,
            with_threads: self.with_threads,
            with_level: self.with_level,
            with_target: self.with_target,
            context_activation: self.context_activation,
            sem_conv_config: self.sem_conv_config,
            with_context: WithContext {
                with_context: OpenTelemetryLayer::<S, Tracer>::get_context,
                with_activated_context: OpenTelemetryLayer::<S, Tracer>::get_activated_context,
                with_activated_otel_context:
                    OpenTelemetryLayer::<S, Tracer>::get_activated_otel_context,
            },
            _registry: self._registry,
            // cannot use ``..self` here due to different generics
        }
    }

    /// Sets whether or not span and event metadata should include OpenTelemetry
    /// exception fields such as `exception.message` and `exception.backtrace`
    /// when an `Error` value is recorded. If multiple error values are recorded
    /// on the same span/event, only the most recently recorded error value will
    /// show up under these fields.
    ///
    /// These attributes follow the [OpenTelemetry semantic conventions for
    /// exceptions][conv].
    ///
    /// By default, these attributes are recorded.
    /// Note that this only works for `(dyn Error + 'static)`.
    /// See [Implementations on Foreign Types of tracing::Value][impls] or [`OpenTelemetryLayer::with_error_events_to_exceptions`]
    ///
    /// [conv]: https://github.com/open-telemetry/semantic-conventions/tree/main/docs/exceptions/
    /// [impls]: https://docs.rs/tracing/0.1.37/tracing/trait.Value.html#foreign-impls
    pub fn with_error_fields_to_exceptions(self, error_fields_to_exceptions: bool) -> Self {
        Self {
            sem_conv_config: SemConvConfig {
                error_fields_to_exceptions,
                ..self.sem_conv_config
            },
            ..self
        }
    }

    /// Sets whether or not an event considered for exception mapping (see [`OpenTelemetryLayer::with_error_records_to_exceptions`])
    /// should be propagated to the span status error description.
    ///
    /// By default, these events do set the span status error description.
    pub fn with_error_events_to_status(self, error_events_to_status: bool) -> Self {
        Self {
            sem_conv_config: SemConvConfig {
                error_events_to_status,
                ..self.sem_conv_config
            },
            ..self
        }
    }

    /// Sets whether or not a subset of events following the described schema are mapped to
    /// events following the [OpenTelemetry semantic conventions for
    /// exceptions][conv].
    ///
    /// * Only events without a message field (unnamed events) and at least one field with the name error
    ///   are considered for mapping.
    ///
    /// By default, these events are mapped.
    ///
    /// [conv]: https://github.com/open-telemetry/semantic-conventions/tree/main/docs/exceptions/
    pub fn with_error_events_to_exceptions(self, error_events_to_exceptions: bool) -> Self {
        Self {
            sem_conv_config: SemConvConfig {
                error_events_to_exceptions,
                ..self.sem_conv_config
            },
            ..self
        }
    }

    /// Sets whether or not reporting an `Error` value on an event will
    /// propagate the OpenTelemetry exception fields such as `exception.message`
    /// and `exception.backtrace` to the corresponding span. You do not need to
    /// enable `with_exception_fields` in order to enable this. If multiple
    /// error values are recorded on the same span/event, only the most recently
    /// recorded error value will show up under these fields.
    ///
    /// These attributes follow the [OpenTelemetry semantic conventions for
    /// exceptions][conv].
    ///
    /// By default, these attributes are propagated to the span. Note that this only works for `(dyn Error + 'static)`.
    /// See [Implementations on Foreign Types of tracing::Value][impls] or [`OpenTelemetryLayer::with_error_events_to_exceptions`]
    ///
    /// [conv]: https://github.com/open-telemetry/semantic-conventions/tree/main/docs/exceptions/
    /// [impls]: https://docs.rs/tracing/0.1.37/tracing/trait.Value.html#foreign-impls
    pub fn with_error_records_to_exceptions(self, error_records_to_exceptions: bool) -> Self {
        Self {
            sem_conv_config: SemConvConfig {
                error_records_to_exceptions,
                ..self.sem_conv_config
            },
            ..self
        }
    }

    /// Sets whether or not span and event metadata should include OpenTelemetry
    /// attributes with location information, such as the file, module and line number.
    ///
    /// These attributes follow the [OpenTelemetry semantic conventions for
    /// source locations][conv].
    ///
    /// By default, locations are enabled.
    ///
    /// [conv]: https://github.com/open-telemetry/semantic-conventions/blob/main/docs/general/attributes.md#source-code-attributes/
    pub fn with_location(self, location: bool) -> Self {
        Self { location, ..self }
    }

    /// Sets whether or not spans metadata should include the _busy time_
    /// (total time for which it was entered), and _idle time_ (total time
    /// the span existed but was not entered).
    ///
    /// By default, inactivity tracking is enabled.
    pub fn with_tracked_inactivity(self, tracked_inactivity: bool) -> Self {
        Self {
            tracked_inactivity,
            ..self
        }
    }

    /// Sets whether or not spans record additional attributes for the thread
    /// name and thread ID of the thread they were created on, following the
    /// [OpenTelemetry semantic conventions for threads][conv].
    ///
    /// By default, thread attributes are enabled.
    ///
    /// [conv]: https://github.com/open-telemetry/semantic-conventions/blob/main/docs/general/attributes.md#general-thread-attributes/
    pub fn with_threads(self, threads: bool) -> Self {
        Self {
            with_threads: threads,
            ..self
        }
    }

    /// Sets whether or not span metadata should include the `tracing` verbosity level information as a `level` field.
    ///
    /// The level is always added to events, and based on [`OpenTelemetryLayer::with_error_events_to_status`]
    /// error-level events will mark the span status as an error.
    ///
    /// By default, level information is disabled.
    pub fn with_level(self, level: bool) -> Self {
        Self {
            with_level: level,
            ..self
        }
    }

    /// Sets whether or not span metadata should include an attribute with `target` from `tracing` spans.
    ///
    /// By default, the target attribute is enabled..
    pub fn with_target(self, target: bool) -> Self {
        Self {
            with_target: target,
            ..self
        }
    }

    /// Sets whether or not an OpenTelemetry Context should be activated on span entry.
    ///
    /// When enabled, entering a span will activate its OpenTelemetry context, making it
    /// available to other OpenTelemetry instrumentation. This allows for proper context
    /// propagation across different instrumentation libraries.
    ///
    /// By default, context activation is enabled.
    pub fn with_context_activation(self, context_activation: bool) -> Self {
        Self {
            context_activation,
            ..self
        }
    }

    /// Adds a filter for events that counts all events that came to the layer. See
    /// [`FilteredOpenTelemetryLayer`] for more details.
    ///
    /// If you just want to filter the events out and you don't need to count how many happened, you
    /// can use [`Layer::with_filter`] instead.
    pub fn with_counting_event_filter<F: Filter<S>>(
        self,
        filter: F,
    ) -> FilteredOpenTelemetryLayer<S, T, F> {
        FilteredOpenTelemetryLayer::new(self, filter)
    }

    /// Retrieve the parent OpenTelemetry [`Context`] from the current tracing
    /// [`span`] through the [`Registry`]. This [`Context`] links spans to their
    /// parent for proper hierarchical visualization.
    ///
    /// [`Context`]: opentelemetry::Context
    /// [`span`]: tracing::Span
    /// [`Registry`]: tracing_subscriber::Registry
    fn parent_context(&self, attrs: &Attributes<'_>, ctx: &Context<'_, S>) -> OtelContext {
        if let Some(parent) = attrs.parent() {
            // A span can have an _explicit_ parent that is NOT seen by this `Layer` (for which
            // `Context::span` returns `None`. This happens if the parent span is filtered away
            // from the layer by a per-layer filter. In that case, we fall-through to the `else`
            // case, and consider this span a root span.
            //
            // This is likely rare, as most users who use explicit parents will configure their
            // filters so that children and parents are both seen, but it's not guaranteed. Also,
            // if users configure their filter with a `reload` filter, it's possible that a parent
            // and child have different filters as they are created with a filter change
            // in-between.
            //
            // In these case, we prefer to emit a smaller span tree instead of panicking.
            if let Some(span) = ctx.span(parent) {
                let mut extensions = span.extensions_mut();
                if let Some(otel_data) = extensions.get_mut::<OtelData>() {
                    // If the parent span has a span builder the parent span should be started
                    // so we get a proper context with the parent span.
                    return self.with_started_cx(otel_data, &|cx| cx.clone());
                }
            }
        }

        if attrs.is_contextual() {
            if self.context_activation {
                // If the span is contextual and we are using context activation,
                // we should use the current OTel context
                OtelContext::current()
            } else {
                // If the span is contextual and we are not using context activation,
                // we should use the current tracing context
                ctx.lookup_current()
                    .and_then(|span| {
                        let mut extensions = span.extensions_mut();
                        extensions
                            .get_mut::<OtelData>()
                            .map(|data| self.with_started_cx(data, &|cx| cx.clone()))
                    })
                    .unwrap_or_else(OtelContext::current)
            }
        } else {
            OtelContext::default()
        }
    }

    /// Provides access to the OpenTelemetry data (`OtelData`) stored in a tracing span.
    ///
    /// This function retrieves the span from the subscriber's registry using the provided span ID,
    /// and then applies the callback function `f` to the span's `OtelData` if present.
    ///
    /// # Parameters
    /// * `dispatch` - A reference to the tracing dispatch, used to access the subscriber
    /// * `id` - The ID of the span to look up
    /// * `f` - A callback function that receives a mutable reference to the span's `OtelData`
    ///   This callback is used to manipulate or extract information from the OpenTelemetry context
    ///   associated with the tracing span
    ///
    fn get_context(dispatch: &tracing::Dispatch, id: &span::Id, f: &mut dyn FnMut(&mut OtelData)) {
        let subscriber = dispatch
            .downcast_ref::<S>()
            .expect("subscriber should downcast to expected type; this is a bug!");
        let span = subscriber
            .span(id)
            .expect("registry should have a span for the current ID");

        let mut extensions = span.extensions_mut();
        if let Some(otel_data) = extensions.get_mut::<OtelData>() {
            f(otel_data);
        }
    }

    /// Retrieves the OpenTelemetry data for a span and activates its context before calling
    /// the provided function.
    ///
    /// This function retrieves the span from the subscriber's registry using the provided
    /// span ID, activates the OTel `Context` in the span's `OtelData` if present, and then
    /// applies the callback function `f` to the `OtelData`.
    ///
    /// # Parameters
    ///
    /// * `dispatch` - The tracing dispatch to downcast and retrieve the span from
    /// * `id` - The span ID to look up in the registry
    /// * `f` - The closure to invoke with mutable access to the span's `OtelData`
    fn get_activated_context(
        dispatch: &tracing::Dispatch,
        id: &span::Id,
        f: &mut dyn FnMut(&mut OtelData),
    ) {
        let subscriber = dispatch
            .downcast_ref::<S>()
            .expect("subscriber should downcast to expected type; this is a bug!");
        let span = subscriber
            .span(id)
            .expect("registry should have a span for the current ID");

        let mut extensions = span.extensions_mut();

        Self::get_activated_context_extensions(dispatch, &mut extensions, f)
    }

    /// Retrieves the activated OpenTelemetry context from a span's extensions and passes it
    /// to the provided function.
    ///
    /// This method activates the context and extracts the current OTel `Context` from the
    /// `OtelData` state if present, and then applies the callback function `f` to a reference
    /// to the OTel `Context`.
    ///
    /// # Parameters
    ///
    /// * `dispatch` - The tracing dispatch to downcast to the `OpenTelemetryLayer`
    /// * `extensions` - Mutable reference to the span's extensions containing `OtelData`
    /// * `f` - The closure to invoke with a reference to the OTel `Context`
    fn get_activated_otel_context(
        dispatch: &tracing::Dispatch,
        extensions: &mut ExtensionsMut<'_>,
        f: &mut dyn FnMut(&OtelContext),
    ) {
        Self::get_activated_context_extensions(
            dispatch,
            extensions,
            &mut |otel_data: &mut OtelData| {
                if let OtelDataState::Context { current_cx } = &otel_data.state {
                    f(current_cx)
                }
            },
        );
    }

    fn get_activated_context_extensions(
        dispatch: &tracing::Dispatch,
        extensions: &mut ExtensionsMut<'_>,
        f: &mut dyn FnMut(&mut OtelData),
    ) {
        let layer = dispatch
            .downcast_ref::<OpenTelemetryLayer<S, T>>()
            .expect("layer should downcast to expected type; this is a bug!");

        if let Some(otel_data) = extensions.get_mut::<OtelData>() {
            // Activate the context
            layer.start_cx(otel_data);
            f(otel_data);
        }
    }

    fn extra_span_attrs(&self) -> usize {
        let mut extra_attrs = 0;
        if self.location {
            extra_attrs += 3;
        }
        if self.with_threads {
            extra_attrs += 2;
        }
        if self.with_level {
            extra_attrs += 1;
        }
        if self.with_target {
            extra_attrs += 1;
        }
        extra_attrs
    }

    ///
    /// Builds the OTel span associated with given OTel context, consuming the SpanBuilder within
    /// the context in the process.
    ///
    fn start_cx(&self, otel_data: &mut OtelData) {
        if let OtelDataState::Context { .. } = &otel_data.state {
            // If the context is already started, we do nothing.
        } else if let OtelDataState::Builder {
            builder,
            parent_cx,
            status,
        } = take(&mut otel_data.state)
        {
            let mut span = builder.start_with_context(&self.tracer, &parent_cx);
            span.set_status(status);
            let current_cx = parent_cx.with_span(span);
            otel_data.state = OtelDataState::Context { current_cx };
        }
    }

    fn with_started_cx<U>(&self, otel_data: &mut OtelData, f: &dyn Fn(&OtelContext) -> U) -> U {
        self.start_cx(otel_data);
        match &otel_data.state {
            OtelDataState::Context { current_cx, .. } => f(current_cx),
            _ => panic!("OtelDataState should be a Context after starting it; this is a bug!"),
        }
    }
}

thread_local! {
    static THREAD_ID: u64 = {
        // OpenTelemetry's semantic conventions require the thread ID to be
        // recorded as an integer, but `std::thread::ThreadId` does not expose
        // the integer value on stable, so we have to convert it to a `usize` by
        // parsing it. Since this requires allocating a `String`, store it in a
        // thread local so we only have to do this once.
        // TODO(eliza): once `std::thread::ThreadId::as_u64` is stabilized
        // (https://github.com/rust-lang/rust/issues/67939), just use that.
        thread_id_integer(thread::current().id())
    };
}

thread_local! {
    static GUARD_STACK: RefCell<IdContextGuardStack> = RefCell::new(IdContextGuardStack::new());
}

type IdContextGuardStack = IdValueStack<ContextGuard>;

impl<S, T> Layer<S> for OpenTelemetryLayer<S, T>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    T: otel::Tracer + 'static,
    T::Span: Send + Sync,
{
    /// Creates an [OpenTelemetry `Span`] for the corresponding [tracing `Span`].
    ///
    /// [OpenTelemetry `Span`]: opentelemetry::trace::Span
    /// [tracing `Span`]: tracing::Span
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if self.tracked_inactivity && extensions.get_mut::<Timings>().is_none() {
            extensions.insert(Timings::new());
        }

        let parent_cx = self.parent_context(attrs, &ctx);
        let mut builder = self
            .tracer
            .span_builder(attrs.metadata().name())
            .with_start_time(crate::time::now());

        let builder_attrs = builder.attributes.get_or_insert(Vec::with_capacity(
            attrs.fields().len() + self.extra_span_attrs(),
        ));

        if self.location {
            let meta = attrs.metadata();

            if let Some(filename) = meta.file() {
                builder_attrs.push(KeyValue::new("code.file.path", filename));
            }

            if let Some(module) = meta.module_path() {
                builder_attrs.push(KeyValue::new("code.module.name", module));
            }

            if let Some(line) = meta.line() {
                builder_attrs.push(KeyValue::new("code.line.number", line as i64));
            }
        }

        if self.with_threads {
            THREAD_ID.with(|id| builder_attrs.push(KeyValue::new("thread.id", *id as i64)));
            if let Some(name) = std::thread::current().name() {
                // TODO(eliza): it's a bummer that we have to allocate here, but
                // we can't easily get the string as a `static`. it would be
                // nice if `opentelemetry` could also take `Arc<str>`s as
                // `String` values...
                builder_attrs.push(KeyValue::new("thread.name", name.to_string()));
            }
        }

        if self.with_level {
            builder_attrs.push(KeyValue::new("level", attrs.metadata().level().as_str()));
        }
        if self.with_target {
            builder_attrs.push(KeyValue::new("target", attrs.metadata().target()));
        }

        let mut updates = SpanBuilderUpdates::default();
        attrs.record(&mut SpanAttributeVisitor {
            span_builder_updates: &mut updates,
            sem_conv_config: self.sem_conv_config,
        });

        let mut status = Status::Unset;
        updates.update(&mut builder, &mut status);
        extensions.insert(OtelData {
            state: OtelDataState::Builder {
                builder,
                parent_cx,
                status,
            },
            end_time: None,
        });
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if !self.context_activation && !self.tracked_inactivity {
            return;
        }

        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if self.context_activation {
            if let Some(otel_data) = extensions.get_mut::<OtelData>() {
                self.with_started_cx(otel_data, &|cx| {
                    let guard = cx.clone().attach();
                    GUARD_STACK.with(|stack| stack.borrow_mut().push(id.clone(), guard));
                });
            }

            if !self.tracked_inactivity {
                return;
            }
        }

        if let Some(timings) = extensions.get_mut::<Timings>() {
            if timings.entered_count == 0 {
                let now = Instant::now();
                timings.idle += (now - timings.last).as_nanos() as i64;
                timings.last = now;
            }
            timings.entered_count += 1;
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(otel_data) = extensions.get_mut::<OtelData>() {
            otel_data.end_time = Some(crate::time::now());
            if self.context_activation {
                GUARD_STACK.with(|stack| stack.borrow_mut().pop(id));
            }
        }

        if !self.tracked_inactivity {
            return;
        }

        if let Some(timings) = extensions.get_mut::<Timings>() {
            timings.entered_count -= 1;
            if timings.entered_count == 0 {
                let now = Instant::now();
                timings.busy += (now - timings.last).as_nanos() as i64;
                timings.last = now;
            }
        }
    }

    /// Record OpenTelemetry [`attributes`] for the given values.
    ///
    /// [`attributes`]: opentelemetry::trace::SpanBuilder::attributes
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut updates = SpanBuilderUpdates::default();
        values.record(&mut SpanAttributeVisitor {
            span_builder_updates: &mut updates,
            sem_conv_config: self.sem_conv_config,
        });
        let mut extensions = span.extensions_mut();
        if let Some(otel_data) = extensions.get_mut::<OtelData>() {
            match &mut otel_data.state {
                OtelDataState::Builder {
                    builder, status, ..
                } => {
                    // If the builder is present, then update it.
                    updates.update(builder, status);
                }
                OtelDataState::Context { current_cx, .. } => {
                    // If the Context has been created, then update the span.
                    updates.update_span(&current_cx.span());
                }
            }
        }
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();
        let Some(data) = extensions.get_mut::<OtelData>() else {
            return; // The span must already have been closed by us
        };

        // The follows span may be filtered away (or closed), from this layer,
        // in which case we just drop the data, as opposed to panicking. This
        // uses the same reasoning as `parent_context` above.
        if let Some(follows_span) = ctx.span(follows) {
            let mut follows_extensions = follows_span.extensions_mut();
            let Some(follows_data) = follows_extensions.get_mut::<OtelData>() else {
                return; // The span must already have been closed by us
            };
            let follows_context =
                self.with_started_cx(follows_data, &|cx| cx.span().span_context().clone());
            match &mut data.state {
                OtelDataState::Builder { builder, .. } => {
                    if let Some(ref mut links) = builder.links {
                        links.push(otel::Link::with_context(follows_context));
                    } else {
                        builder.links = Some(vec![otel::Link::with_context(follows_context)]);
                    }
                }
                OtelDataState::Context { current_cx, .. } => {
                    current_cx.span().add_link(follows_context, vec![]);
                }
            }
        }
    }

    /// Records OpenTelemetry [`Event`] data on event.
    ///
    /// Note: an [`ERROR`]-level event will also set the OpenTelemetry span status code to
    /// [`Error`], signaling that an error has occurred.
    ///
    /// [`Event`]: opentelemetry::trace::Event
    /// [`ERROR`]: tracing::Level::ERROR
    /// [`Error`]: opentelemetry::trace::StatusCode::Error
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // Ignore events that are not in the context of a span
        if let Some(span) = event.parent().and_then(|id| ctx.span(id)).or_else(|| {
            event
                .is_contextual()
                .then(|| ctx.lookup_current())
                .flatten()
        }) {
            // Performing read operations before getting a write lock to avoid a deadlock
            // See https://github.com/tokio-rs/tracing/issues/763
            #[cfg(feature = "tracing-log")]
            let normalized_meta = event.normalized_metadata();
            #[cfg(feature = "tracing-log")]
            let meta = normalized_meta.as_ref().unwrap_or_else(|| event.metadata());
            #[cfg(not(feature = "tracing-log"))]
            let meta = event.metadata();

            let target = Key::new("target");

            #[cfg(feature = "tracing-log")]
            let target = if normalized_meta.is_some() {
                KeyValue::new(target, Value::String(meta.target().to_owned().into()))
            } else {
                KeyValue::new(target, Value::String(event.metadata().target().into()))
            };

            #[cfg(not(feature = "tracing-log"))]
            let target = KeyValue::new(target, Value::String(meta.target().into()));

            let mut otel_event = otel::Event::new(
                String::new(),
                crate::time::now(),
                vec![
                    KeyValue::new(
                        Key::new("level"),
                        Value::String(meta.level().as_str().into()),
                    ),
                    target,
                ],
                0,
            );

            let mut builder_updates = None;
            event.record(&mut SpanEventVisitor {
                event_builder: &mut otel_event,
                span_builder_updates: &mut builder_updates,
                sem_conv_config: self.sem_conv_config,
            });

            // If the event name is still empty, then there was no special handling of error fields.
            // It should be safe to set the event name to the name provided by tracing.
            // This is a hack but there are existing hacks that depend on the name being empty, so to avoid breaking those the event name is set here.
            // Ideally, the name should be set above when the event is constructed.
            // see: https://github.com/tokio-rs/tracing-opentelemetry/pull/28
            if otel_event.name.is_empty() {
                otel_event.name = std::borrow::Cow::Borrowed(event.metadata().name());
            }

            let mut extensions = span.extensions_mut();

            if let Some(otel_data) = extensions.get_mut::<OtelData>() {
                if self.location {
                    #[cfg(not(feature = "tracing-log"))]
                    let normalized_meta: Option<tracing_core::Metadata<'_>> = None;
                    let (file, module) = match &normalized_meta {
                        Some(meta) => (
                            meta.file().map(|s| Value::from(s.to_owned())),
                            meta.module_path().map(|s| Value::from(s.to_owned())),
                        ),
                        None => (
                            event.metadata().file().map(Value::from),
                            event.metadata().module_path().map(Value::from),
                        ),
                    };

                    if let Some(file) = file {
                        otel_event
                            .attributes
                            .push(KeyValue::new("code.file.path", file));
                    }
                    if let Some(module) = module {
                        otel_event
                            .attributes
                            .push(KeyValue::new("code.module.name", module));
                    }
                    if let Some(line) = meta.line() {
                        otel_event
                            .attributes
                            .push(KeyValue::new("code.line.number", line as i64));
                    }
                }

                match &mut otel_data.state {
                    OtelDataState::Builder {
                        builder, status, ..
                    } => {
                        if *status == otel::Status::Unset
                            && *meta.level() == tracing_core::Level::ERROR
                        {
                            *status = otel::Status::error("");
                        }
                        if let Some(builder_updates) = builder_updates {
                            builder_updates.update(builder, status);
                        }
                        if let Some(ref mut events) = builder.events {
                            events.push(otel_event);
                        } else {
                            builder.events = Some(vec![otel_event]);
                        }
                    }
                    OtelDataState::Context { current_cx, .. } => {
                        let span = current_cx.span();
                        // TODO:ban fix this with accessor in SpanRef that can check the span status
                        if *meta.level() == tracing_core::Level::ERROR {
                            span.set_status(otel::Status::error(""));
                        }
                        if let Some(builder_updates) = builder_updates {
                            builder_updates.update_span(&span);
                        }
                        span.add_event(otel_event.name, otel_event.attributes);
                    }
                }
            };
        }
    }

    /// Exports an OpenTelemetry [`Span`] on close.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        // Now get mutable extensions for removal
        let (otel_data, timings) = {
            let mut extensions = span.extensions_mut();
            let timings = if self.tracked_inactivity {
                extensions.remove::<Timings>()
            } else {
                None
            };
            (extensions.remove::<OtelData>(), timings)
        };

        if let Some(OtelData { state, end_time }) = otel_data {
            // Append busy/idle timings when enabled.
            let timings = timings.map(|timings| {
                let busy_ns = Key::new("busy_ns");
                let idle_ns = Key::new("idle_ns");

                vec![
                    KeyValue::new(busy_ns, timings.busy),
                    KeyValue::new(idle_ns, timings.idle),
                ]
            });

            match state {
                OtelDataState::Builder {
                    builder,
                    parent_cx,
                    status,
                } => {
                    // Don't create the context here just to get a SpanRef since it's costly
                    let mut span = builder.start_with_context(&self.tracer, &parent_cx);
                    if let Some(timings) = timings {
                        span.set_attributes(timings)
                    };
                    span.set_status(status);
                    if let Some(end_time) = end_time {
                        span.end_with_timestamp(end_time);
                    } else {
                        span.end();
                    }
                }
                OtelDataState::Context { current_cx } => {
                    let span = current_cx.span();
                    if let Some(timings) = timings {
                        span.set_attributes(timings)
                    };
                    end_time
                        .map_or_else(|| span.end(), |end_time| span.end_with_timestamp(end_time));
                }
            }
        }
    }

    // SAFETY: this is safe because the `WithContext` function pointer is valid
    // for the lifetime of `&self`.
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        match id {
            id if id == TypeId::of::<Self>() => Some(self as *const _ as *const ()),
            id if id == TypeId::of::<WithContext>() => {
                Some(&self.with_context as *const _ as *const ())
            }
            _ => None,
        }
    }
}

struct Timings {
    idle: i64,
    busy: i64,
    last: Instant,
    entered_count: u64,
}

impl Timings {
    fn new() -> Self {
        Self {
            idle: 0,
            busy: 0,
            last: Instant::now(),
            entered_count: 0,
        }
    }
}

fn thread_id_integer(id: thread::ThreadId) -> u64 {
    let thread_id = format!("{id:?}");
    thread_id
        .trim_start_matches("ThreadId(")
        .trim_end_matches(')')
        .parse::<u64>()
        .expect("thread ID should parse as an integer")
}

#[cfg(test)]
mod tests {
    use crate::OpenTelemetrySpanExt;

    use super::*;
    use opentelemetry::trace::{SpanContext, TraceFlags, TracerProvider};
    use opentelemetry_sdk::trace::SpanExporter;
    use std::{collections::HashMap, error::Error, fmt::Display, time::SystemTime};
    use tracing::trace_span;
    use tracing_core::LevelFilter;
    use tracing_subscriber::prelude::*;

    #[derive(Debug, Clone)]
    struct TestTracer {
        tracer: opentelemetry_sdk::trace::Tracer,
        exporter: opentelemetry_sdk::trace::InMemorySpanExporter,
    }

    impl TestTracer {
        fn spans(&mut self) -> Vec<opentelemetry_sdk::trace::SpanData> {
            self.exporter
                .force_flush()
                .expect("problems flushing spans");
            self.exporter
                .get_finished_spans()
                .expect("problems recording spans")
        }

        fn with_data<T>(&mut self, f: impl FnOnce(&opentelemetry_sdk::trace::SpanData) -> T) -> T {
            let spans = self.spans();
            f(spans.first().expect("no spans recorded"))
        }

        fn attributes(&mut self) -> HashMap<String, Value> {
            self.with_data(|data| {
                data.attributes
                    .iter()
                    .map(|kv| (kv.key.to_string(), kv.value.clone()))
                    .collect()
            })
        }
    }

    impl Default for TestTracer {
        fn default() -> Self {
            let exporter = opentelemetry_sdk::trace::InMemorySpanExporter::default();
            let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_simple_exporter(exporter.clone())
                .build();
            let tracer = provider.tracer("test-tracer");
            Self { tracer, exporter }
        }
    }

    impl opentelemetry::trace::Tracer for TestTracer {
        type Span = opentelemetry_sdk::trace::Span;

        fn build_with_context(&self, builder: SpanBuilder, parent_cx: &OtelContext) -> Self::Span {
            self.tracer.build_with_context(builder, parent_cx)
        }
    }

    #[derive(Debug, Clone)]
    struct TestSpan(otel::SpanContext);
    impl otel::Span for TestSpan {
        fn add_event_with_timestamp<T: Into<Cow<'static, str>>>(
            &mut self,
            _: T,
            _: SystemTime,
            _: Vec<KeyValue>,
        ) {
        }
        fn span_context(&self) -> &otel::SpanContext {
            &self.0
        }
        fn is_recording(&self) -> bool {
            false
        }
        fn set_attribute(&mut self, _attribute: KeyValue) {}
        fn set_status(&mut self, _status: otel::Status) {}
        fn update_name<T: Into<Cow<'static, str>>>(&mut self, _new_name: T) {}
        fn add_link(&mut self, _span_context: SpanContext, _attributes: Vec<KeyValue>) {}
        fn end_with_timestamp(&mut self, _timestamp: SystemTime) {}
    }

    #[derive(Debug)]
    struct TestDynError {
        msg: &'static str,
        source: Option<Box<TestDynError>>,
    }
    impl Display for TestDynError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.msg)
        }
    }
    impl Error for TestDynError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match &self.source {
                Some(source) => Some(source),
                None => None,
            }
        }
    }
    impl TestDynError {
        fn new(msg: &'static str) -> Self {
            Self { msg, source: None }
        }
        fn with_parent(self, parent_msg: &'static str) -> Self {
            Self {
                msg: parent_msg,
                source: Some(Box::new(self)),
            }
        }
    }

    #[test]
    fn dynamic_span_names() {
        let dynamic_name = "GET http://example.com".to_string();
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("static_name", otel.name = dynamic_name.as_str());
        });

        let recorded_name = tracer.spans().first().unwrap().name.clone();
        assert_eq!(recorded_name, dynamic_name.as_str())
    }

    #[test]
    fn span_kind() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.kind = "server");
        });

        let recorded_kind = tracer.with_data(|data| data.span_kind.clone());
        assert_eq!(recorded_kind, otel::SpanKind::Server)
    }

    #[test]
    fn span_status_code() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.status_code = ?otel::Status::Ok);
        });

        let recorded_status = tracer.with_data(|data| data.status.clone());
        assert_eq!(recorded_status, otel::Status::Ok)
    }

    #[test]
    fn span_status_description() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        let message = "message";

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.status_description = message);
        });

        let recorded_status_message = tracer.with_data(|data| data.status.clone());

        assert_eq!(recorded_status_message, otel::Status::error(message))
    }

    #[test]
    fn trace_id_from_existing_context_with_context_activation() {
        trace_id_from_existing_context_impl(true);
    }

    #[test]
    fn trace_id_from_existing_context_no_context_activation() {
        trace_id_from_existing_context_impl(false);
    }

    fn trace_id_from_existing_context_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );
        let trace_id = otel::TraceId::from(42u128);
        let existing_cx = OtelContext::current_with_span(TestSpan(otel::SpanContext::new(
            trace_id,
            otel::SpanId::from(1u64),
            TraceFlags::SAMPLED,
            false,
            Default::default(),
        )));
        let _g = existing_cx.attach();

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.kind = "server");
        });

        let recorded_trace_id = tracer.with_data(|data| data.span_context.trace_id());
        assert_eq!(recorded_trace_id, trace_id)
    }

    #[test]
    fn includes_timings_with_context_activation() {
        includes_timings_impl(true);
    }

    #[test]
    fn includes_timings_no_context_activation() {
        includes_timings_impl(false);
    }

    fn includes_timings_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_tracked_inactivity(true)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert!(attributes.contains_key("idle_ns"));
        assert!(attributes.contains_key("busy_ns"));
    }

    #[test]
    fn records_error_fields_with_context_activation() {
        records_error_fields_impl(true);
    }

    #[test]
    fn records_error_fields_no_context_activation() {
        records_error_fields_impl(false);
    }

    fn records_error_fields_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        let err = TestDynError::new("base error")
            .with_parent("intermediate error")
            .with_parent("user error");

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!(
                "request",
                error = &err as &(dyn std::error::Error + 'static)
            );
        });

        let attributes = tracer.attributes();

        assert_eq!(attributes["error"].as_str(), "user error");
        assert_eq!(
            attributes["error.chain"],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );

        assert_eq!(attributes[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            attributes[FIELD_EXCEPTION_STACKTRACE],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );
    }

    #[test]
    fn records_event_name_with_context_activation() {
        records_event_name_impl(true);
    }

    #[test]
    fn records_event_name_no_context_activation() {
        records_event_name_impl(false);
    }

    fn records_event_name_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("test span").in_scope(|| {
                tracing::event!(tracing::Level::INFO, "event name 1"); // this is equivalent to 'message = "event name 1"'
                tracing::event!(name: "event name 2", tracing::Level::INFO, field1 = "field1");
                tracing::event!(name: "event name 3", tracing::Level::INFO, error = "field2");
                tracing::event!(name: "event name 4", tracing::Level::INFO, message = "field3");
                tracing::event!(name: "event name 5", tracing::Level::INFO, name = "field4");
            });
        });

        let events = tracer.with_data(|data| data.events.clone());

        let mut iter = events.iter();

        assert_eq!(iter.next().unwrap().name, "event name 1");
        assert_eq!(iter.next().unwrap().name, "event name 2");
        assert_eq!(iter.next().unwrap().name, "exception"); // error attribute is handled specially
        assert_eq!(iter.next().unwrap().name, "field3"); // message attribute is handled specially
        assert_eq!(iter.next().unwrap().name, "event name 5"); // name attribute should not conflict with event name.
    }

    #[test]
    fn event_filter_count() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_counting_event_filter(LevelFilter::INFO)
                .with_filter(LevelFilter::DEBUG),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("test span").in_scope(|| {
                tracing::event!(tracing::Level::TRACE, "1");
                tracing::event!(tracing::Level::DEBUG, "2");
                tracing::event!(tracing::Level::INFO, "3");
                tracing::event!(tracing::Level::WARN, "4");
                tracing::event!(tracing::Level::ERROR, "5");
            });
        });

        let events = tracer.with_data(|data| data.events.clone());

        let mut iter = events.iter();

        assert_eq!(iter.next().unwrap().name, "3");
        assert_eq!(iter.next().unwrap().name, "4");
        assert_eq!(iter.next().unwrap().name, "5");
        assert!(iter.next().is_none());

        let spans = tracer.spans();
        assert_eq!(spans.len(), 1);

        let Value::I64(event_count) = spans
            .first()
            .unwrap()
            .attributes
            .iter()
            .find(|key_value| key_value.key.as_str() == SPAN_EVENT_COUNT_FIELD)
            .unwrap()
            .value
        else {
            panic!("Unexpected type of `{SPAN_EVENT_COUNT_FIELD}`.");
        };
        // We've sent 5 events out of which 1 was filtered out by an actual filter and another by
        // our event filter.
        assert_eq!(event_count, 4);
    }

    #[test]
    fn records_no_error_fields_with_context_activation() {
        records_no_error_fields_impl(true);
    }

    #[test]
    fn records_no_error_fields_no_context_activation() {
        records_no_error_fields_impl(false);
    }

    fn records_no_error_fields_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_error_records_to_exceptions(false)
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        let err = TestDynError::new("base error")
            .with_parent("intermediate error")
            .with_parent("user error");

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!(
                "request",
                error = &err as &(dyn std::error::Error + 'static)
            );
        });

        let attributes = tracer.attributes();

        assert_eq!(attributes["error"].as_str(), "user error");
        assert_eq!(
            attributes["error.chain"],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );

        assert_eq!(attributes[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            attributes[FIELD_EXCEPTION_STACKTRACE],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );
    }

    #[test]
    fn includes_span_location_with_context_activation() {
        includes_span_location_impl(true);
    }

    #[test]
    fn includes_span_location_no_context_activation() {
        includes_span_location_impl(false);
    }

    fn includes_span_location_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_location(true)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert!(attributes.contains_key("code.file.path"));
        assert!(attributes.contains_key("code.module.name"));
        assert!(attributes.contains_key("code.line.number"));
    }

    #[test]
    fn excludes_span_location_with_context_activation() {
        excludes_span_location_impl(true);
    }

    #[test]
    fn excludes_span_location_no_context_activation() {
        excludes_span_location_impl(false);
    }

    fn excludes_span_location_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_location(false)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert!(!attributes.contains_key("code.file.path"));
        assert!(!attributes.contains_key("code.module.name"));
        assert!(!attributes.contains_key("code.line.number"));
    }

    #[test]
    fn includes_thread_with_context_activation() {
        includes_thread_impl(true);
    }

    #[test]
    fn includes_thread_no_context_activation() {
        includes_thread_impl(false);
    }

    fn includes_thread_impl(context_activation: bool) {
        let thread = thread::current();
        let expected_name = thread
            .name()
            .map(|name| Value::String(name.to_owned().into()));
        let expected_id = Value::I64(thread_id_integer(thread.id()) as i64);

        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_threads(true)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert_eq!(attributes.get("thread.name"), expected_name.as_ref());
        assert_eq!(attributes.get("thread.id"), Some(&expected_id));
    }

    #[test]
    fn excludes_thread_with_context_activation() {
        excludes_thread_impl(true);
    }

    #[test]
    fn excludes_thread_no_context_activation() {
        excludes_thread_impl(false);
    }

    fn excludes_thread_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_threads(false)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert!(!attributes.contains_key("thread.name"));
        assert!(!attributes.contains_key("thread.id"));
    }

    #[test]
    fn includes_level_with_context_activation() {
        includes_level_impl(true);
    }

    #[test]
    fn includes_level_no_context_activation() {
        includes_level_impl(false);
    }

    fn includes_level_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_level(true)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert!(attributes.contains_key("level"));
    }

    #[test]
    fn excludes_level_with_context_activation() {
        excludes_level_impl(true);
    }

    #[test]
    fn excludes_level_no_context_activation() {
        excludes_level_impl(false);
    }

    fn excludes_level_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_level(false)
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();

        assert!(!attributes.contains_key("level"));
    }

    #[test]
    fn includes_target() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_target(true));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();
        let keys = attributes.keys().map(|k| k.as_str()).collect::<Vec<&str>>();
        assert!(keys.contains(&"target"));
    }

    #[test]
    fn excludes_target() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_target(false));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.attributes();
        let keys = attributes.keys().map(|k| k.as_str()).collect::<Vec<&str>>();
        assert!(!keys.contains(&"target"));
    }

    #[test]
    fn propagates_error_fields_from_event_to_span_with_context_activation() {
        propagates_error_fields_from_event_to_span_impl(true);
    }

    #[test]
    fn propagates_error_fields_from_event_to_span_no_context_activation() {
        propagates_error_fields_from_event_to_span_impl(false);
    }

    fn propagates_error_fields_from_event_to_span_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        let err = TestDynError::new("base error")
            .with_parent("intermediate error")
            .with_parent("user error");

        tracing::subscriber::with_default(subscriber, || {
            let _guard = tracing::debug_span!("request",).entered();

            tracing::error!(
                error = &err as &(dyn std::error::Error + 'static),
                "request error!"
            )
        });

        let attributes = tracer.attributes();

        assert_eq!(attributes[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            attributes[FIELD_EXCEPTION_STACKTRACE],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );
    }

    #[test]
    fn propagates_no_error_fields_from_event_to_span_with_context_activation() {
        propagates_no_error_fields_from_event_to_span_impl(true);
    }

    #[test]
    fn propagates_no_error_fields_from_event_to_span_no_context_activation() {
        propagates_no_error_fields_from_event_to_span_impl(false);
    }

    fn propagates_no_error_fields_from_event_to_span_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_error_fields_to_exceptions(false)
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        let err = TestDynError::new("base error")
            .with_parent("intermediate error")
            .with_parent("user error");

        tracing::subscriber::with_default(subscriber, || {
            let _guard = tracing::debug_span!("request",).entered();

            tracing::error!(
                error = &err as &(dyn std::error::Error + 'static),
                "request error!"
            )
        });

        let attributes = tracer.attributes();

        assert_eq!(attributes[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            attributes[FIELD_EXCEPTION_STACKTRACE],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );
    }

    #[test]
    fn tracing_error_compatibility_with_context_activation() {
        tracing_error_compatibility_impl(true);
    }

    #[test]
    fn tracing_error_compatibility_no_context_activation() {
        tracing_error_compatibility_impl(false);
    }

    fn tracing_error_compatibility_impl(context_activation: bool) {
        let tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry()
            .with(
                layer()
                    .with_error_fields_to_exceptions(false)
                    .with_tracer(tracer.clone())
                    .with_context_activation(context_activation),
            )
            .with(tracing_error::ErrorLayer::default());

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("Blows up!", exception = tracing::field::Empty);
            let _entered = span.enter();
            let context = tracing_error::SpanTrace::capture();

            // This can cause a deadlock if `on_record` locks extensions while attributes are visited
            span.record("exception", tracing::field::debug(&context));
            // This can cause a deadlock if `on_event` locks extensions while the event is visited
            tracing::info!(exception = &tracing::field::debug(&context), "hello");
        });

        // No need to assert anything, as long as this finished (and did not panic), everything is ok.
    }

    #[derive(Debug, PartialEq)]
    struct ValueA(&'static str);
    #[derive(Debug, PartialEq)]
    struct ValueB(&'static str);

    #[test]
    fn otel_context_propagation() {
        use opentelemetry::trace::Tracer;
        use tracing::span;

        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            // Add a value to the current OpenTelemetry context for the bridge to propagate
            let _outer_guard =
                OtelContext::attach(OtelContext::default().with_value(ValueA("outer")));
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));
            let root = span!(tracing::Level::TRACE, "tokio-tracing-span-parent");
            // Drop the guard to ensure the context is cleared
            drop(_outer_guard);
            assert!(OtelContext::current().get::<ValueA>().is_none());
            // Enter the root span, the context should be propagated
            let _enter_root = root.enter();
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));
            // Add another value to the current OpenTelemetry context for the bridge to propagate
            let _inner_guard =
                OtelContext::attach(OtelContext::current_with_value(ValueB("inner")));
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));
            assert_eq!(OtelContext::current().get(), Some(&ValueB("inner")));
            let child = span!(tracing::Level::TRACE, "tokio-tracing-span-child");
            // Drop the guard to ensure the context is reverted
            drop(_inner_guard);
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));
            assert!(OtelContext::current().get::<ValueB>().is_none());
            // Enter the child span, the context should be propagated
            let _enter_child = child.enter();
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));
            assert_eq!(OtelContext::current().get(), Some(&ValueB("inner")));
            // Create an OpenTelemetry span using the OpentTelemetry notion of current
            // span to see check that it is a child of the tokio child span
            let span = tracer
                .tracer
                .span_builder("otel-tracing-span")
                .start(&tracer);
            let _otel_guard = OtelContext::attach(OtelContext::current_with_span(span));
            let child2 = span!(tracing::Level::TRACE, "tokio-tracing-span-child2");
            drop(_otel_guard);
            // Drop the child span, the context should be reverted
            drop(_enter_child);
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));
            assert!(OtelContext::current().get::<ValueB>().is_none());
            // Drop the root span, the context should be reverted
            drop(_enter_root);
            assert!(OtelContext::current().get::<ValueA>().is_none());
            assert!(OtelContext::current().get::<ValueB>().is_none());
            let _ = child2.enter();
        });

        // Let's check the spans
        let spans = tracer.spans();
        let parent = spans
            .iter()
            .find(|span| span.name == "tokio-tracing-span-parent")
            .unwrap();
        let child = spans
            .iter()
            .find(|span| span.name == "tokio-tracing-span-child")
            .unwrap();
        let child2 = spans
            .iter()
            .find(|span| span.name == "tokio-tracing-span-child2")
            .unwrap();
        let otel = spans
            .iter()
            .find(|span| span.name == "otel-tracing-span")
            .unwrap();
        // The tokio parent span should be a root span
        assert_eq!(parent.parent_span_id, otel::SpanId::INVALID);
        // The first tokio child span should have the tokio parent span as parent
        assert_eq!(child.parent_span_id, parent.span_context.span_id());
        // The otel span should have the first tokio child span as parent
        assert_eq!(otel.parent_span_id, child.span_context.span_id());
        // The second tokio child span should have the otel span as parent
        assert_eq!(child2.parent_span_id, otel.span_context.span_id());
    }

    #[test]
    fn parent_context() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            let root = trace_span!("root");

            let child1 = trace_span!("child-1");
            let root_context = root.context(); // SpanData (None)
            let _ = child1.set_parent(root_context); // Clone context, but SpanData(None)

            let _enter_root = root.enter();
            drop(_enter_root);

            let child2 = trace_span!("child-2");
            let _ = child2.set_parent(root.context());
        });

        // Let's check the spans
        let spans = tracer.spans();
        let parent = spans.iter().find(|span| span.name == "root").unwrap();
        let child1 = spans.iter().find(|span| span.name == "child-1").unwrap();
        let child2 = spans.iter().find(|span| span.name == "child-2").unwrap();
        assert_eq!(parent.parent_span_id, otel::SpanId::INVALID);
        assert_eq!(child1.parent_span_id, parent.span_context.span_id());
        assert_eq!(child2.parent_span_id, parent.span_context.span_id());
    }

    #[test]
    fn record_after_with_context_activation() {
        record_after_impl(true);
    }

    #[test]
    fn record_after_no_context_activation() {
        record_after_impl(false);
    }

    fn record_after_impl(context_activation: bool) {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        tracing::subscriber::with_default(subscriber, || {
            let root = trace_span!("root", before = "before", after = "before");

            // Record a value before the span is entered
            root.record("before", "after");

            // Enter and exit the span
            let _enter_root = root.enter();
            drop(_enter_root);

            // Record a value after the span is exited
            root.record("after", "after");
        });

        // Let's check the spans. Both values should've been
        // updated to 'after'.
        let spans = tracer.spans();
        let parent = spans.iter().find(|span| span.name == "root").unwrap();
        assert_eq!(parent.parent_span_id, otel::SpanId::INVALID);
        assert!(parent
            .attributes
            .iter()
            .filter(|kv| kv.key.as_str() == "before")
            .any(|kv| kv.value.as_str() == "after"));

        assert!(parent
            .attributes
            .iter()
            .filter(|kv| kv.key.as_str() == "after")
            .any(|kv| kv.value.as_str() == "after"));
    }

    #[test]
    fn parent_context_2() {
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            let root = trace_span!("root");
            _ = root.enter();

            let child1 = trace_span!("child-1");
            let _ = child1.set_parent(root.context());

            trace_span!(parent: &child1, "child-2");
            let _ = child1.set_parent(root.context()); // <-- this is what causes the issue

            trace_span!(parent: &child1, "child-3");
        });

        // Let's check the spans
        let spans = tracer.spans();
        let root = spans.iter().find(|span| span.name == "root").unwrap();
        let child1 = spans.iter().find(|span| span.name == "child-1").unwrap();
        let child2 = spans.iter().find(|span| span.name == "child-2").unwrap();
        let child3 = spans.iter().find(|span| span.name == "child-3").unwrap();
        assert_eq!(root.parent_span_id, otel::SpanId::INVALID);
        assert_eq!(child1.parent_span_id, root.span_context.span_id());
        assert_eq!(child2.parent_span_id, child1.span_context.span_id());

        // The parent should be `child1`
        assert_eq!(child3.parent_span_id, child1.span_context.span_id());
    }

    #[test]
    fn follows_from_adds_link_with_context_activation() {
        follows_from_adds_link_impl(true);
    }

    #[test]
    fn follows_from_adds_link_no_context_activation() {
        follows_from_adds_link_impl(false);
    }

    fn follows_from_adds_link_impl(context_activation: bool) {
        use crate::OpenTelemetrySpanExt;
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        let span1_id = tracing::subscriber::with_default(subscriber, || {
            let span2 = tracing::debug_span!("span2");
            let span1 = tracing::debug_span!("span1");

            // Ensure that span2 is started
            let _ = span2.context();

            // Establish follows_from relationship
            span2.follows_from(&span1);

            // Enter span2 to ensure it's exported
            let _guard = span2.enter();

            // Get span ID for later verification
            span1.context().span().span_context().span_id()
        });

        let spans = tracer.spans();
        // Check that both spans are exported
        assert_eq!(spans.len(), 2, "Expected two spans to be exported");
        assert!(spans.iter().any(|span| span.name == "span1"));
        let span2 = spans
            .iter()
            .find(|span| span.name == "span2")
            .expect("Expected span2 to be exported");

        // Collect span2 links
        let links = span2
            .links
            .iter()
            .map(|link| link.span_context.span_id())
            .collect::<Vec<_>>();

        // Verify that span2 has a link to span1
        assert_eq!(
            links.len(),
            1,
            "Expected span to have one link from follows_from relationship"
        );

        assert!(
            links.contains(&span1_id),
            "Link should point to the correct source span"
        );
    }

    #[test]
    fn follows_from_multiple_links_with_context_activation() {
        follows_from_multiple_links_impl(true);
    }

    #[test]
    fn follows_from_multiple_links_no_context_activation() {
        follows_from_multiple_links_impl(false);
    }

    fn follows_from_multiple_links_impl(context_activation: bool) {
        use crate::OpenTelemetrySpanExt;
        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(context_activation),
        );

        let (span1_id, span2_id) = tracing::subscriber::with_default(subscriber, || {
            let span3 = tracing::debug_span!("span3");
            let span2 = tracing::debug_span!("span2");
            let span1 = tracing::debug_span!("span1");

            // Establish multiple follows_from relationships
            span3.follows_from(&span1);
            span3.follows_from(&span2);

            // Enter span3 to ensure it's exported
            let _guard = span3.enter();

            // Get span IDs for later verification
            (
                span1.context().span().span_context().span_id(),
                span2.context().span().span_context().span_id(),
            )
        });

        let spans = tracer.spans();
        // Check that all three spans are exported
        assert_eq!(spans.len(), 3, "Expected three spans to be exported");
        assert!(spans.iter().any(|span| span.name == "span1"));
        assert!(spans.iter().any(|span| span.name == "span2"));
        let span3 = spans
            .iter()
            .find(|span| span.name == "span3")
            .expect("Expected span3 to be exported");

        // Collect span3 links
        let links = span3
            .links
            .iter()
            .map(|link| link.span_context.span_id())
            .collect::<Vec<_>>();

        // Verify that span3 has multiple links and they point to the correct spans
        assert_eq!(
            links.len(),
            2,
            "Expected span to have two links from follows_from relationships"
        );

        // Verify that the links point to the correct spans in the correct order
        assert!(
            links[0] == span1_id && links[1] == span2_id,
            "Links should point to the correct source spans"
        );
    }

    #[test]
    fn context_activation_disabled() {
        use tracing::span;

        let mut tracer = TestTracer::default();
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_context_activation(false),
        );

        tracing::subscriber::with_default(subscriber, || {
            // Add a value to the current OpenTelemetry context
            let _outer_guard =
                OtelContext::attach(OtelContext::default().with_value(ValueA("outer")));
            assert_eq!(OtelContext::current().get(), Some(&ValueA("outer")));

            let root = span!(tracing::Level::TRACE, "tokio-tracing-span-parent");

            // Drop the guard to ensure the context is cleared
            drop(_outer_guard);
            assert!(OtelContext::current().get::<ValueA>().is_none());

            // Enter the root span - with context activation disabled,
            // the context should NOT be propagated
            let _enter_root = root.enter();
            assert!(OtelContext::current().get::<ValueA>().is_none());

            // Add another value to the current OpenTelemetry context
            let _inner_guard =
                OtelContext::attach(OtelContext::current_with_value(ValueB("inner")));
            assert!(OtelContext::current().get::<ValueA>().is_none());
            assert_eq!(OtelContext::current().get(), Some(&ValueB("inner")));

            let child = span!(tracing::Level::TRACE, "tokio-tracing-span-child");

            // Drop the guard to ensure the context is reverted
            drop(_inner_guard);
            assert!(OtelContext::current().get::<ValueA>().is_none());
            assert!(OtelContext::current().get::<ValueB>().is_none());

            // Enter the child span - with context activation disabled,
            // the context should NOT be propagated
            let _enter_child = child.enter();
            assert!(OtelContext::current().get::<ValueA>().is_none());
            assert!(OtelContext::current().get::<ValueB>().is_none());
        });

        // Verify spans were still created and exported
        let spans = tracer.spans();
        assert_eq!(spans.len(), 2);
        assert!(spans
            .iter()
            .any(|span| span.name == "tokio-tracing-span-parent"));
        assert!(spans
            .iter()
            .any(|span| span.name == "tokio-tracing-span-child"));
    }
}
