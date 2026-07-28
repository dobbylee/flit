import Foundation

enum FoundationCopyKey: String {
    case dashboardActivity = "dashboard.activity"
    case dashboardActivityUnknown = "dashboard.activity.unknown"
    case dashboardAttention = "dashboard.attention"
    case dashboardChanges = "dashboard.changes"
    case dashboardChangesUnavailable = "dashboard.changes.unavailable"
    case dashboardDataUnavailable = "dashboard.data.unavailable"
    case dashboardNoRuns = "dashboard.noRuns"
    case dashboardSectionFinished = "dashboard.section.finished"
    case dashboardSectionNeedsAttention = "dashboard.section.needsAttention"
    case dashboardSectionPossiblyStuck = "dashboard.section.possiblyStuck"
    case dashboardSectionWorking = "dashboard.section.working"
    case dashboardUnavailable = "dashboard.unavailable"
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
        .dashboardActivity: "Activity: %@ · %d%% confidence",
        .dashboardActivityUnknown: "Activity: Unknown",
        .dashboardAttention: "Attention: %@ · %llu open",
        .dashboardChanges: "Changes: %llu files · +%llu −%llu",
        .dashboardChangesUnavailable: "Changes unavailable: %@",
        .dashboardDataUnavailable: "Dashboard data unavailable",
        .dashboardNoRuns: "No Runs",
        .dashboardSectionFinished: "Finished",
        .dashboardSectionNeedsAttention: "Needs Attention",
        .dashboardSectionPossiblyStuck: "Possibly Stuck",
        .dashboardSectionWorking: "Working",
        .dashboardUnavailable: "Dashboard unavailable",
        .boundaryChecking:
            "Verifying the local Core and Store. Provider monitoring has not started.",
        .boundaryReady:
            "The local Core and Store are ready. Provider monitoring has not started.",
        .boundaryUnavailable:
            "Flit could not open its local Core and Store safely. No agent controls are available.",
        .local: "Local by design",
        .noControls: "No agent controls yet",
        .phase: "Flit · Phase 2",
        .statusChecking: "Checking foundation",
        .statusReady: "Core and Store ready",
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

    static func format(_ key: FoundationCopyKey, _ arguments: CVarArg...) -> String {
        String(format: text(key), arguments: arguments)
    }
}
