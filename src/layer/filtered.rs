use std::any::TypeId;

use opentelemetry::{trace::TraceContextExt as _, Key, KeyValue, Value};
use tracing::{span, Event, Subscriber};
use tracing_subscriber::{
    layer::{Context, Filter},
    registry::LookupSpan,
    Layer,
};

use crate::{OtelData, OtelDataState};

use super::{OpenTelemetryLayer, SPAN_EVENT_COUNT_FIELD};

/// A layer wrapping a [`OpenTelemetryLayer`], discarding all events filtered out by a given
/// [`Filter`]. This can be built by calling [`OpenTelemetryLayer::with_counting_event_filter`].
///
/// Only events that are not filtered out will be saved as events on the span. All events, including
/// those filtered out, will be counted and the total will be provided in the
/// `otel.tracing_event_count` field of the exported span.
///
/// This is useful when there is large volume of logs outputted by the application and it would be
/// too expensive to export all of them as span events, but it is still desirable to have
/// information whether there is more information in logs for the given span.
pub struct FilteredOpenTelemetryLayer<S, T, F> {
    inner: OpenTelemetryLayer<S, T>,
    filter: F,
}

impl<S, T, F> FilteredOpenTelemetryLayer<S, T, F> {
    pub fn map_inner<Mapper, S2, T2>(self, mapper: Mapper) -> FilteredOpenTelemetryLayer<S2, T2, F>
    where
        Mapper: FnOnce(OpenTelemetryLayer<S, T>) -> OpenTelemetryLayer<S2, T2>,
        F: Filter<S>,
    {
        FilteredOpenTelemetryLayer {
            inner: mapper(self.inner),
            filter: self.filter,
        }
    }

    pub fn with_counting_event_filter<F2>(self, filter: F2) -> FilteredOpenTelemetryLayer<S, T, F2>
    where
        F2: Filter<S>,
    {
        FilteredOpenTelemetryLayer {
            inner: self.inner,
            filter,
        }
    }

    pub(crate) fn new(inner: OpenTelemetryLayer<S, T>, filter: F) -> Self
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
        F: Filter<S>,
    {
        Self { inner, filter }
    }
}

struct EventCount(u32);

impl<S, T, F> Layer<S> for FilteredOpenTelemetryLayer<S, T, F>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    OpenTelemetryLayer<S, T>: Layer<S>,
    F: Filter<S> + 'static,
{
    fn on_layer(&mut self, subscriber: &mut S) {
        self.inner.on_layer(subscriber);
    }

    fn register_callsite(
        &self,
        metadata: &'static tracing::Metadata<'static>,
    ) -> tracing_core::Interest {
        self.inner.register_callsite(metadata)
    }

    fn enabled(&self, metadata: &tracing::Metadata<'_>, ctx: Context<'_, S>) -> bool {
        self.inner.enabled(metadata, ctx)
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        self.inner.on_new_span(attrs, id, ctx);
    }

    fn on_record(&self, span: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        self.inner.on_record(span, values, ctx);
    }

    fn on_follows_from(&self, span: &span::Id, follows: &span::Id, ctx: Context<'_, S>) {
        self.inner.on_follows_from(span, follows, ctx);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(span) = event.parent().and_then(|id| ctx.span(id)).or_else(|| {
            event
                .is_contextual()
                .then(|| ctx.lookup_current())
                .flatten()
        }) else {
            return;
        };

        {
            let mut extensions = span.extensions_mut();

            if let Some(count) = extensions.get_mut::<EventCount>() {
                count.0 += 1;
            } else {
                extensions.insert(EventCount(1));
            }
        }

        drop(span);

        println!("evaluating event with level {}", event.metadata().level());
        if self.filter.enabled(event.metadata(), &ctx) {
            println!("processing event with level {}", event.metadata().level());
            self.inner.on_event(event, ctx);
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        self.inner.on_enter(id, ctx);
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        self.inner.on_exit(id, ctx);
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let count = extensions.remove::<EventCount>().map_or(0, |count| count.0);
        if let Some(OtelData { state, end_time: _ }) = extensions.get_mut::<OtelData>() {
            let key_value = KeyValue::new(
                Key::from_static_str(SPAN_EVENT_COUNT_FIELD),
                Value::I64(i64::from(count)),
            );
            match state {
                OtelDataState::Builder {
                    builder,
                    parent_cx: _,
                    status: _,
                } => {
                    builder.attributes.get_or_insert(Vec::new()).push(key_value);
                }
                OtelDataState::Context { current_cx } => {
                    let span = current_cx.span();
                    span.set_attribute(key_value);
                }
            }
        }

        drop(extensions);
        drop(span);

        self.inner.on_close(id, ctx);
    }

    fn on_id_change(&self, old: &span::Id, new: &span::Id, ctx: Context<'_, S>) {
        self.inner.on_id_change(old, new, ctx);
    }

    /// SAFETY: this is sound as long as the inner implementation is sound.
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            Some(self as *const _ as *const ())
        } else {
            unsafe { self.inner.downcast_raw(id) }
        }
    }

    // `and_then`, `with_subscriber`, and `with_filter` are not implemented on purpose. Other
    // methods should probably be implemented manually if there are new provided methods.
}
