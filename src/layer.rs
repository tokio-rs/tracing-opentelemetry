use crate::{OtelData, PreSampledTracer};
use once_cell::unsync;
use opentelemetry::{
    trace::{self as otel, noop, SpanBuilder, SpanKind, Status, TraceContextExt},
    Context as OtelContext, Key, KeyValue, StringValue, Value,
};
use std::fmt;
use std::marker;
use std::thread;
#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"))))]
use std::time::Instant;
use std::{any::TypeId, borrow::Cow};
use tracing_core::span::{self, Attributes, Id, Record};
use tracing_core::{field, Event, Subscriber};
#[cfg(feature = "tracing-log")]
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
#[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
use web_time::Instant;

const SPAN_NAME_FIELD: &str = "otel.name";
const SPAN_KIND_FIELD: &str = "otel.kind";
const SPAN_STATUS_CODE_FIELD: &str = "otel.status_code";
const SPAN_STATUS_DESCRIPTION_FIELD: &str = "otel.status_description";

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
    sem_conv_config: SemConvConfig,
    get_context: WithContext,
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

// this function "remembers" the types of the subscriber so that we
// can downcast to something aware of them without knowing those
// types at the callsite.
//
// See https://github.com/tokio-rs/tracing/blob/4dad420ee1d4607bad79270c1520673fa6266a3d/tracing-error/src/layer.rs
pub(crate) struct WithContext(
    #[allow(clippy::type_complexity)]
    fn(&tracing::Dispatch, &span::Id, f: &mut dyn FnMut(&mut OtelData, &dyn PreSampledTracer)),
);

impl WithContext {
    // This function allows a function to be called in the context of the
    // "remembered" subscriber.
    pub(crate) fn with_context(
        &self,
        dispatch: &tracing::Dispatch,
        id: &span::Id,
        mut f: impl FnMut(&mut OtelData, &dyn PreSampledTracer),
    ) {
        (self.0)(dispatch, id, &mut f)
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
    fn update(self, span_builder: &mut SpanBuilder) {
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
            span_builder.status = status;
        }
        if let Some(attributes) = attributes {
            if let Some(builder_attributes) = &mut span_builder.attributes {
                builder_attributes.extend(attributes);
            } else {
                span_builder.attributes = Some(attributes);
            }
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
                        .replace(otel::Status::error(format!("{:?}", value)));
                }
                if self.sem_conv_config.error_events_to_exceptions {
                    self.event_builder.name = EVENT_EXCEPTION_NAME.into();
                    self.event_builder.attributes.push(KeyValue::new(
                        FIELD_EXCEPTION_MESSAGE,
                        format!("{:?}", value),
                    ));
                } else {
                    self.event_builder
                        .attributes
                        .push(KeyValue::new("error", format!("{:?}", value)));
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
            "message" => self.event_builder.name = format!("{:?}", value).into(),
            // While tracing supports the error primitive, the instrumentation macro does not
            // use the primitive and instead uses the debug or display primitive.
            // In both cases, an event with an empty name and with an error attribute is created.
            "error" if self.event_builder.name.is_empty() => {
                if self.sem_conv_config.error_events_to_status {
                    self.span_builder_updates
                        .get_or_insert_with(SpanBuilderUpdates::default)
                        .status
                        .replace(otel::Status::error(format!("{:?}", value)));
                }
                if self.sem_conv_config.error_events_to_exceptions {
                    self.event_builder.name = EVENT_EXCEPTION_NAME.into();
                    self.event_builder.attributes.push(KeyValue::new(
                        FIELD_EXCEPTION_MESSAGE,
                        format!("{:?}", value),
                    ));
                } else {
                    self.event_builder
                        .attributes
                        .push(KeyValue::new("error", format!("{:?}", value)));
                }
            }
            // Skip fields that are actually log metadata that have already been handled
            #[cfg(feature = "tracing-log")]
            name if name.starts_with("log.") => (),
            name => {
                self.event_builder
                    .attributes
                    .push(KeyValue::new(name, format!("{:?}", value)));
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
            SPAN_NAME_FIELD => self.span_builder_updates.name = Some(format!("{:?}", value).into()),
            SPAN_KIND_FIELD => {
                self.span_builder_updates.span_kind = str_to_span_kind(&format!("{:?}", value))
            }
            SPAN_STATUS_CODE_FIELD => {
                self.span_builder_updates.status = Some(str_to_status(&format!("{:?}", value)))
            }
            SPAN_STATUS_DESCRIPTION_FIELD => {
                self.span_builder_updates.status = Some(otel::Status::error(format!("{:?}", value)))
            }
            _ => self.record(KeyValue::new(
                Key::new(field.name()),
                Value::String(format!("{:?}", value).into()),
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
    T: otel::Tracer + PreSampledTracer + 'static,
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
            sem_conv_config: SemConvConfig {
                error_fields_to_exceptions: true,
                error_records_to_exceptions: true,
                error_events_to_exceptions: true,
                error_events_to_status: true,
            },

            get_context: WithContext(Self::get_context),
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
        Tracer: otel::Tracer + PreSampledTracer + 'static,
    {
        OpenTelemetryLayer {
            tracer,
            location: self.location,
            tracked_inactivity: self.tracked_inactivity,
            with_threads: self.with_threads,
            with_level: self.with_level,
            sem_conv_config: self.sem_conv_config,
            get_context: WithContext(OpenTelemetryLayer::<S, Tracer>::get_context),
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

    /// Sets whether or not an event considered for exception mapping (see [`OpenTelemetryLayer::with_error_recording`])
    /// should be propagated to the span status error description.
    ///
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
                return extensions
                    .get_mut::<OtelData>()
                    .map(|builder| self.tracer.sampled_context(builder))
                    .unwrap_or_default();
            }
        }

        // Else if the span is inferred from context, look up any available current span.
        if attrs.is_contextual() {
            ctx.lookup_current()
                .and_then(|span| {
                    let mut extensions = span.extensions_mut();
                    extensions
                        .get_mut::<OtelData>()
                        .map(|builder| self.tracer.sampled_context(builder))
                })
                .unwrap_or_else(OtelContext::current)
        // Explicit root spans should have no parent context.
        } else {
            OtelContext::new()
        }
    }

    fn get_context(
        dispatch: &tracing::Dispatch,
        id: &span::Id,
        f: &mut dyn FnMut(&mut OtelData, &dyn PreSampledTracer),
    ) {
        let subscriber = dispatch
            .downcast_ref::<S>()
            .expect("subscriber should downcast to expected type; this is a bug!");
        let span = subscriber
            .span(id)
            .expect("registry should have a span for the current ID");
        let layer = dispatch
            .downcast_ref::<OpenTelemetryLayer<S, T>>()
            .expect("layer should downcast to expected type; this is a bug!");

        let mut extensions = span.extensions_mut();
        if let Some(builder) = extensions.get_mut::<OtelData>() {
            f(builder, &layer.tracer);
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
        extra_attrs
    }
}

thread_local! {
    static THREAD_ID: unsync::Lazy<u64> = unsync::Lazy::new(|| {
        // OpenTelemetry's semantic conventions require the thread ID to be
        // recorded as an integer, but `std::thread::ThreadId` does not expose
        // the integer value on stable, so we have to convert it to a `usize` by
        // parsing it. Since this requires allocating a `String`, store it in a
        // thread local so we only have to do this once.
        // TODO(eliza): once `std::thread::ThreadId::as_u64` is stabilized
        // (https://github.com/rust-lang/rust/issues/67939), just use that.
        thread_id_integer(thread::current().id())
    });
}

impl<S, T> Layer<S> for OpenTelemetryLayer<S, T>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    T: otel::Tracer + PreSampledTracer + 'static,
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
            .with_start_time(crate::time::now())
            // Eagerly assign span id so children have stable parent id
            .with_span_id(self.tracer.new_span_id());

        // Record new trace id if there is no active parent span
        if !parent_cx.has_active_span() {
            builder.trace_id = Some(self.tracer.new_trace_id());
        }

        let builder_attrs = builder.attributes.get_or_insert(Vec::with_capacity(
            attrs.fields().len() + self.extra_span_attrs(),
        ));

        if self.location {
            let meta = attrs.metadata();

            if let Some(filename) = meta.file() {
                builder_attrs.push(KeyValue::new("code.filepath", filename));
            }

            if let Some(module) = meta.module_path() {
                builder_attrs.push(KeyValue::new("code.namespace", module));
            }

            if let Some(line) = meta.line() {
                builder_attrs.push(KeyValue::new("code.lineno", line as i64));
            }
        }

        if self.with_threads {
            THREAD_ID.with(|id| builder_attrs.push(KeyValue::new("thread.id", **id as i64)));
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

        let mut updates = SpanBuilderUpdates::default();
        attrs.record(&mut SpanAttributeVisitor {
            span_builder_updates: &mut updates,
            sem_conv_config: self.sem_conv_config,
        });

        updates.update(&mut builder);
        extensions.insert(OtelData { builder, parent_cx });
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if !self.tracked_inactivity {
            return;
        }

        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

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
            otel_data.builder.end_time = Some(crate::time::now());
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
        if let Some(data) = extensions.get_mut::<OtelData>() {
            updates.update(&mut data.builder);
        }
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();
        let data = extensions
            .get_mut::<OtelData>()
            .expect("Missing otel data span extensions");

        // The follows span may be filtered away (or closed), from this layer,
        // in which case we just drop the data, as opposed to panicking. This
        // uses the same reasoning as `parent_context` above.
        if let Some(follows_span) = ctx.span(follows) {
            let mut follows_extensions = follows_span.extensions_mut();
            let follows_data = follows_extensions
                .get_mut::<OtelData>()
                .expect("Missing otel data span extensions");

            let follows_context = self
                .tracer
                .sampled_context(follows_data)
                .span()
                .span_context()
                .clone();
            let follows_link = otel::Link::with_context(follows_context);
            if let Some(ref mut links) = data.builder.links {
                links.push(follows_link);
            } else {
                data.builder.links = Some(vec![follows_link]);
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
            let otel_data = extensions.get_mut::<OtelData>();

            if let Some(otel_data) = otel_data {
                let builder = &mut otel_data.builder;

                if builder.status == otel::Status::Unset
                    && *meta.level() == tracing_core::Level::ERROR
                {
                    builder.status = otel::Status::error("")
                }

                if let Some(builder_updates) = builder_updates {
                    builder_updates.update(builder);
                }

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
                            .push(KeyValue::new("code.filepath", file));
                    }
                    if let Some(module) = module {
                        otel_event
                            .attributes
                            .push(KeyValue::new("code.namespace", module));
                    }
                    if let Some(line) = meta.line() {
                        otel_event
                            .attributes
                            .push(KeyValue::new("code.lineno", line as i64));
                    }
                }

                if let Some(ref mut events) = builder.events {
                    events.push(otel_event);
                } else {
                    builder.events = Some(vec![otel_event]);
                }
            }
        };
    }

    /// Exports an OpenTelemetry [`Span`] on close.
    ///
    /// [`Span`]: opentelemetry::trace::Span
    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let (otel_data, timings) = {
            let mut extensions = span.extensions_mut();
            let timings = if self.tracked_inactivity {
                extensions.remove::<Timings>()
            } else {
                None
            };
            (extensions.remove::<OtelData>(), timings)
        };

        if let Some(OtelData {
            mut builder,
            parent_cx,
        }) = otel_data
        {
            // Append busy/idle timings when enabled.
            if let Some(timings) = timings {
                let busy_ns = Key::new("busy_ns");
                let idle_ns = Key::new("idle_ns");

                let attributes = builder
                    .attributes
                    .get_or_insert_with(|| Vec::with_capacity(2));
                attributes.push(KeyValue::new(busy_ns, timings.busy));
                attributes.push(KeyValue::new(idle_ns, timings.idle));
            }

            // Build and start span, drop span to export
            builder.start_with_context(&self.tracer, &parent_cx);
        }
    }

    // SAFETY: this is safe because the `WithContext` function pointer is valid
    // for the lifetime of `&self`.
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        match id {
            id if id == TypeId::of::<Self>() => Some(self as *const _ as *const ()),
            id if id == TypeId::of::<WithContext>() => {
                Some(&self.get_context as *const _ as *const ())
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
    let thread_id = format!("{:?}", id);
    thread_id
        .trim_start_matches("ThreadId(")
        .trim_end_matches(')')
        .parse::<u64>()
        .expect("thread ID should parse as an integer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{SpanContext, TraceFlags};
    use std::{
        collections::HashMap,
        error::Error,
        fmt::Display,
        sync::{Arc, Mutex},
        time::SystemTime,
    };
    use tracing_subscriber::prelude::*;

    #[derive(Debug, Clone)]
    struct TestTracer(Arc<Mutex<Option<OtelData>>>);
    impl otel::Tracer for TestTracer {
        type Span = noop::NoopSpan;
        fn start_with_context<T>(&self, _name: T, _context: &OtelContext) -> Self::Span
        where
            T: Into<Cow<'static, str>>,
        {
            noop::NoopSpan::DEFAULT
        }
        fn span_builder<T>(&self, name: T) -> otel::SpanBuilder
        where
            T: Into<Cow<'static, str>>,
        {
            otel::SpanBuilder::from_name(name)
        }
        fn build_with_context(
            &self,
            builder: otel::SpanBuilder,
            parent_cx: &OtelContext,
        ) -> Self::Span {
            *self.0.lock().unwrap() = Some(OtelData {
                builder,
                parent_cx: parent_cx.clone(),
            });
            noop::NoopSpan::DEFAULT
        }
    }

    impl PreSampledTracer for TestTracer {
        fn sampled_context(&self, _builder: &mut crate::OtelData) -> OtelContext {
            OtelContext::new()
        }
        fn new_trace_id(&self) -> otel::TraceId {
            otel::TraceId::INVALID
        }
        fn new_span_id(&self) -> otel::SpanId {
            otel::SpanId::INVALID
        }
    }

    impl TestTracer {
        fn with_data<T>(&self, f: impl FnOnce(&OtelData) -> T) -> T {
            let lock = self.0.lock().unwrap();
            let data = lock.as_ref().expect("no span data has been recorded yet");
            f(data)
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
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("static_name", otel.name = dynamic_name.as_str());
        });

        let recorded_name = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .map(|b| b.builder.name.clone());
        assert_eq!(recorded_name, Some(dynamic_name.into()))
    }

    #[test]
    fn span_kind() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.kind = "server");
        });

        let recorded_kind = tracer.with_data(|data| data.builder.span_kind.clone());
        assert_eq!(recorded_kind, Some(otel::SpanKind::Server))
    }

    #[test]
    fn span_status_code() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.status_code = ?otel::Status::Ok);
        });

        let recorded_status = tracer.with_data(|data| data.builder.status.clone());
        assert_eq!(recorded_status, otel::Status::Ok)
    }

    #[test]
    fn span_status_description() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        let message = "message";

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.status_description = message);
        });

        let recorded_status_message = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .builder
            .status
            .clone();

        assert_eq!(recorded_status_message, otel::Status::error(message))
    }

    #[test]
    fn trace_id_from_existing_context() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));
        let trace_id = otel::TraceId::from(42u128);
        let existing_cx = OtelContext::current_with_span(TestSpan(otel::SpanContext::new(
            trace_id,
            otel::SpanId::from(1u64),
            TraceFlags::default(),
            false,
            Default::default(),
        )));
        let _g = existing_cx.attach();

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request", otel.kind = "server");
        });

        let recorded_trace_id =
            tracer.with_data(|data| data.parent_cx.span().span_context().trace_id());
        assert_eq!(recorded_trace_id, trace_id)
    }

    #[test]
    fn includes_timings() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_tracer(tracer.clone())
                .with_tracked_inactivity(true),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.with_data(|data| data.builder.attributes.as_ref().unwrap().clone());
        let keys = attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect::<Vec<&str>>();
        assert!(keys.contains(&"idle_ns"));
        assert!(keys.contains(&"busy_ns"));
    }

    #[test]
    fn records_error_fields() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        let err = TestDynError::new("base error")
            .with_parent("intermediate error")
            .with_parent("user error");

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!(
                "request",
                error = &err as &(dyn std::error::Error + 'static)
            );
        });

        let attributes = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .builder
            .attributes
            .as_ref()
            .unwrap()
            .clone();

        let key_values = attributes
            .into_iter()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value))
            .collect::<HashMap<_, _>>();

        assert_eq!(key_values["error"].as_str(), "user error");
        assert_eq!(
            key_values["error.chain"],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );

        assert_eq!(key_values[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            key_values[FIELD_EXCEPTION_STACKTRACE],
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
    fn records_event_name() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("test span").in_scope(|| {
                tracing::event!(tracing::Level::INFO, "event name 1"); // this is equivalent to 'message = "event name 1"'
                tracing::event!(name: "event name 2", tracing::Level::INFO, field1 = "field1");
                tracing::event!(name: "event name 3", tracing::Level::INFO, error = "field2");
                tracing::event!(name: "event name 4", tracing::Level::INFO, message = "field3");
                tracing::event!(name: "event name 5", tracing::Level::INFO, name = "field4");
            });
        });

        let events = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .builder
            .events
            .as_ref()
            .unwrap()
            .clone();

        let mut iter = events.iter();

        assert_eq!(iter.next().unwrap().name, "event name 1");
        assert_eq!(iter.next().unwrap().name, "event name 2");
        assert_eq!(iter.next().unwrap().name, "exception"); // error attribute is handled specially
        assert_eq!(iter.next().unwrap().name, "field3"); // message attribute is handled specially
        assert_eq!(iter.next().unwrap().name, "event name 5"); // name attribute should not conflict with event name.
    }

    #[test]
    fn records_no_error_fields() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_error_records_to_exceptions(false)
                .with_tracer(tracer.clone()),
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

        let attributes = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .builder
            .attributes
            .as_ref()
            .unwrap()
            .clone();

        let key_values = attributes
            .into_iter()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value))
            .collect::<HashMap<_, _>>();

        assert_eq!(key_values["error"].as_str(), "user error");
        assert_eq!(
            key_values["error.chain"],
            Value::Array(
                vec![
                    StringValue::from("intermediate error"),
                    StringValue::from("base error")
                ]
                .into()
            )
        );

        assert_eq!(key_values[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            key_values[FIELD_EXCEPTION_STACKTRACE],
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
    fn includes_span_location() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_location(true));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.with_data(|data| data.builder.attributes.as_ref().unwrap().clone());
        let keys = attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect::<Vec<&str>>();
        assert!(keys.contains(&"code.filepath"));
        assert!(keys.contains(&"code.namespace"));
        assert!(keys.contains(&"code.lineno"));
    }

    #[test]
    fn excludes_span_location() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_location(false));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.with_data(|data| data.builder.attributes.as_ref().unwrap().clone());
        let keys = attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect::<Vec<&str>>();
        assert!(!keys.contains(&"code.filepath"));
        assert!(!keys.contains(&"code.namespace"));
        assert!(!keys.contains(&"code.lineno"));
    }

    #[test]
    fn includes_thread() {
        let thread = thread::current();
        let expected_name = thread
            .name()
            .map(|name| Value::String(name.to_owned().into()));
        let expected_id = Value::I64(thread_id_integer(thread.id()) as i64);

        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_threads(true));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer
            .with_data(|data| data.builder.attributes.as_ref().unwrap().clone())
            .drain(..)
            .map(|kv| (kv.key.as_str().to_string(), kv.value))
            .collect::<HashMap<_, _>>();
        assert_eq!(attributes.get("thread.name"), expected_name.as_ref());
        assert_eq!(attributes.get("thread.id"), Some(&expected_id));
    }

    #[test]
    fn excludes_thread() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_threads(false));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.with_data(|data| data.builder.attributes.as_ref().unwrap().clone());
        let keys = attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect::<Vec<&str>>();
        assert!(!keys.contains(&"thread.name"));
        assert!(!keys.contains(&"thread.id"));
    }

    #[test]
    fn includes_level() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_level(true));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.with_data(|data| data.builder.attributes.as_ref().unwrap().clone());
        let keys = attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect::<Vec<&str>>();
        assert!(keys.contains(&"level"));
    }

    #[test]
    fn excludes_level() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()).with_level(false));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("request");
        });

        let attributes = tracer.with_data(|data| data.builder.attributes.as_ref().unwrap().clone());
        let keys = attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect::<Vec<&str>>();
        assert!(!keys.contains(&"level"));
    }

    #[test]
    fn propagates_error_fields_from_event_to_span() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

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

        let attributes = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .builder
            .attributes
            .as_ref()
            .unwrap()
            .clone();

        let key_values = attributes
            .into_iter()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value))
            .collect::<HashMap<_, _>>();

        assert_eq!(key_values[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            key_values[FIELD_EXCEPTION_STACKTRACE],
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
    fn propagates_no_error_fields_from_event_to_span() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_error_fields_to_exceptions(false)
                .with_tracer(tracer.clone()),
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

        let attributes = tracer
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .builder
            .attributes
            .as_ref()
            .unwrap()
            .clone();

        let key_values = attributes
            .into_iter()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value))
            .collect::<HashMap<_, _>>();

        assert_eq!(key_values[FIELD_EXCEPTION_MESSAGE].as_str(), "user error");
        assert_eq!(
            key_values[FIELD_EXCEPTION_STACKTRACE],
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
    fn tracing_error_compatibility() {
        let tracer = TestTracer(Arc::new(Mutex::new(None)));
        let subscriber = tracing_subscriber::registry()
            .with(
                layer()
                    .with_error_fields_to_exceptions(false)
                    .with_tracer(tracer.clone()),
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
}
