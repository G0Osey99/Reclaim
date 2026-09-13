import Foundation
import ReclaimCore

/// Bridges the Rust `EventSink` callback (invoked on a background thread) onto a
/// caller-supplied handler, hopping to the main queue so it is safe to mutate
/// `@Published` UI state (doc 08 §4 "never block the UI on the core").
public final class EventForwarder: EventSink, @unchecked Sendable {
    private let handler: (RcEvent) -> Void

    public init(onMain handler: @escaping (RcEvent) -> Void) {
        self.handler = handler
    }

    public func onEvent(event: RcEvent) {
        DispatchQueue.main.async { [handler] in
            handler(event)
        }
    }
}
