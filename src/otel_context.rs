use crate::layer::WithContext;
use tracing::Dispatch;
use tracing_subscriber::registry::ExtensionsMut;

/// Utility functions to allow tracing [`ExtensionsMut`]s to return
/// [OpenTelemetry] [`Context`]s.
///
/// [`ExtensionsMut`]: tracing_subscriber::registry::ExtensionsMut
/// [OpenTelemetry]: https://opentelemetry.io
/// [`Context`]: opentelemetry::Context
///
/// Extracts the OpenTelemetry [`Context`] associated with this span extensions.
///
/// This method retrieves the OpenTelemetry context data that has been stored
/// for the span by the OpenTelemetry layer. The context includes the span's
/// OpenTelemetry span context, which contains trace ID, span ID, and other
/// trace-related metadata.
///
/// [`Context`]: opentelemetry::Context
///
/// # Examples
///
/// ```rust
/// use tracing_opentelemetry::get_otel_context;
/// use tracing::dispatcher::WeakDispatch;
/// use tracing_subscriber::registry::LookupSpan;
/// use opentelemetry::trace::TraceContextExt;
///
/// fn do_things_with_otel_context<'a, D>(
///     span_ref: &tracing_subscriber::registry::SpanRef<'a, D>,
///     weak_dispatch: &WeakDispatch
/// ) where
///     D: LookupSpan<'a>,
/// {
///     if let Some(dispatch) = weak_dispatch.upgrade() {
///         if let Some(otel_context) = get_otel_context(&mut span_ref.extensions_mut(), &dispatch) {
///             // Process the extracted context
///             let span = otel_context.span();
///             let span_context = span.span_context();
///             if span_context.is_valid() {
///                 // Handle the valid context...
///             }
///         }
///     }
/// }
/// ```
///
/// # Use Cases
///
/// - When working with multiple subscriber configurations
/// - When implementing advanced tracing middleware that manages multiple dispatches
pub fn get_otel_context(
    extensions: &mut ExtensionsMut<'_>,
    dispatch: &Dispatch,
) -> Option<opentelemetry::Context> {
    let mut cx = None;
    if let Some(get_context) = dispatch.downcast_ref::<WithContext>() {
        // If our span hasn't been built, we should build it and get the context in one call
        get_context.with_activated_otel_context(
            dispatch,
            extensions,
            |current_cx: &opentelemetry::Context| {
                cx = Some(current_cx.clone());
            },
        );
    }
    cx
}
