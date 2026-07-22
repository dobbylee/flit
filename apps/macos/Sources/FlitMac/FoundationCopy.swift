import Foundation

enum FoundationCopyKey: String {
    case boundaryChecking = "foundation.boundary.checking"
    case boundaryReady = "foundation.boundary.ready"
    case boundaryUnavailable = "foundation.boundary.unavailable"
    case local = "foundation.local"
    case noControls = "foundation.noControls"
    case phase = "foundation.phase"
    case statusChecking = "foundation.status.checking"
    case statusReady = "foundation.status.ready"
    case statusUnavailable = "foundation.status.unavailable"
    case summary = "foundation.summary"
    case title = "foundation.title"
}

enum FoundationCopy {
    private static let values: [FoundationCopyKey: String] = [
        .boundaryChecking:
            "Verifying the local Core contract. Storage and provider monitoring have not started.",
        .boundaryReady:
            "The local Core contract is ready. Storage and provider monitoring have not started.",
        .boundaryUnavailable:
            "Flit could not verify the expected foundation state. No agent controls are available.",
        .local: "Local by design",
        .noControls: "No agent controls yet",
        .phase: "Flit · Phase 1",
        .statusChecking: "Checking foundation",
        .statusReady: "Core contract verified",
        .statusUnavailable: "Foundation unavailable",
        .summary: "A quiet home for the moments that need your attention.",
        .title: "Flit foundation",
    ]

    static func text(_ key: FoundationCopyKey) -> String {
        guard let value = values[key] else {
            preconditionFailure("Missing foundation copy for \(key.rawValue)")
        }
        return value
    }
}
