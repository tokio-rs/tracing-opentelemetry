use crate::OtelData;
use crate::{layer::WithContext, OtelDataState};
use opentelemetry::{
    time,
    trace::{SpanContext, Status, TraceContextExt},
    Context, Key, KeyValue, Value,
};
use std::{borrow::Cow, fmt, time::SystemTime};

/// Utility functions to allow tracing [`Span`]s to accept and return
/// [OpenTelemetry] [`Context`]s.
///
/// [`Span`]: tracing::Span
/// [OpenTelemetry]: https://opentelemetry.io
/// [`Context`]: opentelemetry::Context
pub trait OpenTelemetrySpanExt {
    /// Associates `self` with a given OpenTelemetry trace, using the provided
    /// parent [`Context`].
    ///
    /// This method exists primarily to make it possible to inject a _distributed_ incoming
    /// context, e.g. span IDs, etc.
    ///
    /// A span's parent should only be set _once_, for the purpose described above.
    /// Additionally, once a span has been fully built - and the SpanBuilder has been
    /// consumed - the parent _cannot_ be mutated.
    ///
    /// This method provides error handling for cases where the span context
    /// cannot be set, such as when the OpenTelemetry layer is not present
    /// or when the span has already been started.
    ///
    /// [`Context`]: opentelemetry::Context
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::{propagation::TextMapPropagator, trace::TraceContextExt};
    /// use opentelemetry_sdk::propagation::TraceContextPropagator;
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use std::collections::HashMap;
    /// use tracing::Span;
    ///
    /// // Example carrier, could be a framework header map that impls otel's `Extractor`.
    /// let mut carrier = HashMap::new();
    ///
    /// // Propagator can be swapped with b3 propagator, jaeger propagator, etc.
    /// let propagator = TraceContextPropagator::new();
    ///
    /// // Extract otel parent context via the chosen propagator
    /// let parent_context = propagator.extract(&carrier);
    ///
    /// // Generate a tracing span as usual
    /// let app_root = tracing::span!(tracing::Level::INFO, "app_start");
    ///
    /// // Assign parent trace from external context
    /// let _ = app_root.set_parent(parent_context.clone());
    ///
    /// // Or if the current span has been created elsewhere:
    /// let _ = Span::current().set_parent(parent_context);
    /// ```
    fn set_parent(&self, cx: Context) -> Result<(), SetParentError>;

    /// Associates `self` with a given OpenTelemetry trace, using the provided
    /// followed span [`SpanContext`].
    ///
    /// [`SpanContext`]: opentelemetry::trace::SpanContext
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::{propagation::TextMapPropagator, trace::TraceContextExt};
    /// use opentelemetry_sdk::propagation::TraceContextPropagator;
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use std::collections::HashMap;
    /// use tracing::Span;
    ///
    /// // Example carrier, could be a framework header map that impls otel's `Extractor`.
    /// let mut carrier = HashMap::new();
    ///
    /// // Propagator can be swapped with b3 propagator, jaeger propagator, etc.
    /// let propagator = TraceContextPropagator::new();
    ///
    /// // Extract otel context of linked span via the chosen propagator
    /// let linked_span_otel_context = propagator.extract(&carrier);
    ///
    /// // Extract the linked span context from the otel context
    /// let linked_span_context = linked_span_otel_context.span().span_context().clone();
    ///
    /// // Generate a tracing span as usual
    /// let app_root = tracing::span!(tracing::Level::INFO, "app_start");
    ///
    /// // Assign linked trace from external context
    /// app_root.add_link(linked_span_context);
    ///
    /// // Or if the current span has been created elsewhere:
    /// let linked_span_context = linked_span_otel_context.span().span_context().clone();
    /// Span::current().add_link(linked_span_context);
    /// ```
    fn add_link(&self, cx: SpanContext);

    /// Associates `self` with a given OpenTelemetry trace, using the provided
    /// followed span [`SpanContext`] and attributes.
    ///
    /// [`SpanContext`]: opentelemetry::trace::SpanContext
    fn add_link_with_attributes(&self, cx: SpanContext, attributes: Vec<KeyValue>);

    /// Extracts an OpenTelemetry [`Context`] from `self`.
    ///
    /// [`Context`]: opentelemetry::Context
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::Context;
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use tracing::Span;
    ///
    /// fn make_request(cx: Context) {
    ///     // perform external request after injecting context
    ///     // e.g. if the request's headers impl `opentelemetry::propagation::Injector`
    ///     // then `propagator.inject_context(cx, request.headers_mut())`
    /// }
    ///
    /// // Generate a tracing span as usual
    /// let app_root = tracing::span!(tracing::Level::INFO, "app_start");
    ///
    /// // To include tracing context in client requests from _this_ app,
    /// // extract the current OpenTelemetry context.
    /// make_request(app_root.context());
    ///
    /// // Or if the current span has been created elsewhere:
    /// make_request(Span::current().context())
    /// ```
    fn context(&self) -> Context;

    /// Sets an OpenTelemetry attribute directly for this span, bypassing `tracing`.
    /// If fields set here conflict with `tracing` fields, the `tracing` fields will supersede fields set with `set_attribute`.
    /// This allows for more than 32 fields.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::Context;
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use tracing::Span;
    ///
    /// // Generate a tracing span as usual
    /// let app_root = tracing::span!(tracing::Level::INFO, "app_start");
    ///
    /// // Set the `http.request.header.x_forwarded_for` attribute to `example`.
    /// app_root.set_attribute("http.request.header.x_forwarded_for", "example");
    /// ```
    fn set_attribute(&self, key: impl Into<Key>, value: impl Into<Value>);

    /// Sets an OpenTelemetry status for this span.
    /// This is useful for setting the status of a span that was created by a library that does not declare
    /// the otel.status_code field of the span in advance.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::trace::Status;
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use tracing::Span;
    ///
    /// /// // Generate a tracing span as usual
    /// let app_root = tracing::span!(tracing::Level::INFO, "app_start");
    ///
    /// // Set the Status of the span to `Status::Ok`.
    /// app_root.set_status(Status::Ok);
    /// ```            
    fn set_status(&self, status: Status);

    /// Adds an OpenTelemetry event directly to this span, bypassing `tracing::event!`.
    /// This allows for adding events with dynamic attribute keys, similar to `set_attribute` for span attributes.
    /// Events are added with the current timestamp.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::{KeyValue};
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use tracing::Span;
    ///
    /// let app_root = tracing::span!(tracing::Level::INFO, "processing_request");
    ///
    /// let dynamic_attrs = vec![
    ///     KeyValue::new("job_id", "job-123"),
    ///     KeyValue::new("user.id", "user-xyz"),
    /// ];
    ///
    /// // Add event using the extension method
    /// app_root.add_event("job_started".to_string(), dynamic_attrs);
    ///
    /// // ... perform work ...
    ///
    /// app_root.add_event("job_completed", vec![KeyValue::new("status", "success")]);
    /// ```
    fn add_event(&self, name: impl Into<Cow<'static, str>>, attributes: Vec<KeyValue>);

    /// Adds an OpenTelemetry event with a specific timestamp directly to this span.
    /// Similar to `add_event`, but allows overriding the event timestamp.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use opentelemetry::{KeyValue};
    /// use tracing_opentelemetry::OpenTelemetrySpanExt;
    /// use tracing::Span;
    /// use std::time::{Duration, SystemTime};
    /// use std::borrow::Cow;
    ///
    /// let app_root = tracing::span!(tracing::Level::INFO, "historical_event_processing");
    ///
    /// let event_time = SystemTime::now() - Duration::from_secs(60);
    /// let event_attrs = vec![KeyValue::new("record_id", "rec-456")];
    /// let event_name: Cow<'static, str> = "event_from_past".into();
    ///
    /// app_root.add_event_with_timestamp(event_name, event_time, event_attrs);
    /// ```
    fn add_event_with_timestamp(
        &self,
        name: impl Into<Cow<'static, str>>,
        timestamp: SystemTime,
        attributes: Vec<KeyValue>,
    );
}

/// An error returned if [`OpenTelemetrySpanExt::set_parent`] could not set the parent.
#[derive(Debug)]
pub enum SetParentError {
    /// The layer could not be found and therefore the action could not be carried out. This can
    /// happen with some advanced layers that do not handle downcasting well, for example
    /// [`tracing_subscriber::reload::Layer`].
    LayerNotFound,

    /// The span has been already started.
    ///
    /// Someone already called a context-starting method such as [`OpenTelemetrySpanExt::context`]
    /// or the span has been entered and automatic context starting was not configured out.
    AlreadyStarted,

    /// The span is filtered out by tracing filters.
    ///
    /// If the filtered out span had children, they will not be connected to the parent span either.
    SpanDisabled,
}

impl fmt::Display for SetParentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetParentError::LayerNotFound => {
                write!(f, "OpenTelemetry layer not found")
            }
            SetParentError::AlreadyStarted => {
                write!(f, "Span has already been started, cannot set parent")
            }
            SetParentError::SpanDisabled => {
                write!(f, "Span disabled")
            }
        }
    }
}

impl std::error::Error for SetParentError {}

impl OpenTelemetrySpanExt for tracing::Span {
    fn set_parent(&self, cx: Context) -> Result<(), SetParentError> {
        let mut cx = Some(cx);

        self.with_subscriber(move |(id, subscriber)| {
            let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                return Err(SetParentError::LayerNotFound);
            };

            let mut result = Ok(());
            let result_ref = &mut result;
            // Set the parent OTel for the current span
            get_context.with_context(subscriber, id, move |data| {
                let Some(new_cx) = cx.take() else {
                    *result_ref = Err(SetParentError::AlreadyStarted);
                    return;
                };
                // Create a new context with the new parent but preserve our span.
                // NOTE - if the span has been created - if we have _already_
                // consumed our SpanBuilder_ - we can no longer mutate our parent!
                // This is an intentional design decision.
                match &mut data.state {
                    OtelDataState::Builder { parent_cx, .. } => {
                        // If we still have a builder, update the data so it uses the
                        // new parent context when it's eventually built
                        *parent_cx = new_cx;
                    }
                    OtelDataState::Context { .. } => {
                        *result_ref = Err(SetParentError::AlreadyStarted);
                    }
                }
            });
            result
        })
        .unwrap_or(Err(SetParentError::SpanDisabled))
    }

    fn add_link(&self, cx: SpanContext) {
        self.add_link_with_attributes(cx, Vec::new())
    }

    fn add_link_with_attributes(&self, cx: SpanContext, attributes: Vec<KeyValue>) {
        if cx.is_valid() {
            let mut cx = Some(cx);
            let mut att = Some(attributes);
            self.with_subscriber(move |(id, subscriber)| {
                let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                    return;
                };
                get_context.with_context(subscriber, id, move |data| {
                    let Some(cx) = cx.take() else {
                        return;
                    };
                    let attr = att.take().unwrap_or_default();
                    let follows_link = opentelemetry::trace::Link::new(cx, attr, 0);
                    match &mut data.state {
                        OtelDataState::Builder { builder, .. } => {
                            // If we still have a builder, update the data so it uses the
                            // new link when it's eventually built
                            builder
                                .links
                                .get_or_insert_with(|| Vec::with_capacity(1))
                                .push(follows_link);
                        }
                        OtelDataState::Context { current_cx } => {
                            // If we have a context, add the link to the span in the context
                            current_cx
                                .span()
                                .add_link(follows_link.span_context, follows_link.attributes);
                        }
                    }
                });
            });
        }
    }

    fn context(&self) -> Context {
        let mut cx = None;
        self.with_subscriber(|(id, subscriber)| {
            let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                return;
            };
            // If our span hasn't been built, we should build it and get the context in one call
            get_context.with_activated_context(subscriber, id, |data: &mut OtelData| {
                if let OtelDataState::Context { current_cx } = &data.state {
                    cx = Some(current_cx.clone());
                }
            });
        });

        cx.unwrap_or_default()
    }

    fn set_attribute(&self, key: impl Into<Key>, value: impl Into<Value>) {
        self.with_subscriber(move |(id, subscriber)| {
            let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                return;
            };
            let mut key_value = Some(KeyValue::new(key.into(), value.into()));
            get_context.with_context(subscriber, id, move |data| {
                match &mut data.state {
                    OtelDataState::Builder { builder, .. } => {
                        if builder.attributes.is_none() {
                            builder.attributes = Some(Default::default());
                        }
                        builder
                            .attributes
                            .as_mut()
                            .unwrap()
                            .push(key_value.take().unwrap());
                    }
                    OtelDataState::Context { current_cx } => {
                        let span = current_cx.span();
                        span.set_attribute(key_value.take().unwrap());
                    }
                };
            });
        });
    }

    fn set_status(&self, status: Status) {
        self.with_subscriber(move |(id, subscriber)| {
            let mut status = Some(status);
            let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                return;
            };
            get_context.with_context(subscriber, id, move |data| match &mut data.state {
                OtelDataState::Builder { status: s, .. } => {
                    *s = status.take().unwrap();
                }
                OtelDataState::Context { current_cx } => {
                    let span = current_cx.span();
                    span.set_status(status.take().unwrap());
                }
            });
        });
    }

    fn add_event(&self, name: impl Into<Cow<'static, str>>, attributes: Vec<KeyValue>) {
        self.add_event_with_timestamp(name, time::now(), attributes);
    }

    fn add_event_with_timestamp(
        &self,
        name: impl Into<Cow<'static, str>>,
        timestamp: SystemTime,
        attributes: Vec<KeyValue>,
    ) {
        self.with_subscriber(move |(id, subscriber)| {
            let mut event = Some(opentelemetry::trace::Event::new(
                name, timestamp, attributes, 0,
            ));
            let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                return;
            };
            get_context.with_context(subscriber, id, move |data| {
                let Some(event) = event.take() else {
                    return;
                };
                match &mut data.state {
                    OtelDataState::Builder { builder, .. } => {
                        builder
                            .events
                            .get_or_insert_with(|| Vec::with_capacity(1))
                            .push(event);
                    }
                    OtelDataState::Context { current_cx } => {
                        let span = current_cx.span();
                        span.add_event_with_timestamp(
                            event.name,
                            event.timestamp,
                            event.attributes,
                        );
                    }
                }
            });
        });
    }
}
