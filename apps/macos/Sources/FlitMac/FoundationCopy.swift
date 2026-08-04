import Foundation

enum FoundationCopyKey: String {
    case dashboardActivity = "dashboard.activity"
    case dashboardActivityUnknown = "dashboard.activity.unknown"
    case dashboardAttention = "dashboard.attention"
    case dashboardChanges = "dashboard.changes"
    case dashboardChangesObservedDuringRun = "dashboard.changes.observedDuringRun"
    case dashboardChangesUnavailable = "dashboard.changes.unavailable"
    case dashboardDataUnavailable = "dashboard.data.unavailable"
    case dashboardNoRuns = "dashboard.noRuns"
    case dashboardSectionFinished = "dashboard.section.finished"
    case dashboardSectionNeedsAttention = "dashboard.section.needsAttention"
    case dashboardSectionPossiblyStuck = "dashboard.section.possiblyStuck"
    case dashboardSectionWorking = "dashboard.section.working"
    case dashboardUnavailable = "dashboard.unavailable"
    case dashboardViewActivity = "dashboard.viewActivity"
    case closeToTrayAcknowledgement = "lifecycle.closeToTray.acknowledgement"
    case closeToTrayMessage = "lifecycle.closeToTray.message"
    case closeToTrayTitle = "lifecycle.closeToTray.title"
    case boundaryChecking = "foundation.boundary.checking"
    case boundaryReady = "foundation.boundary.ready"
    case boundaryUnavailable = "foundation.boundary.unavailable"
    case local = "foundation.local"
    case menuBarTooltip = "lifecycle.menuBar.tooltip"
    case menuOpen = "lifecycle.menu.open"
    case menuQuit = "lifecycle.menu.quit"
    case quitActiveRuns = "lifecycle.quit.activeRuns"
    case quitCancel = "lifecycle.quit.cancel"
    case quitConfirm = "lifecycle.quit.confirm"
    case quitImpactUnavailable = "lifecycle.quit.impactUnavailable"
    case quitMonitoringBoundary = "lifecycle.quit.monitoringBoundary"
    case quitNoActiveRuns = "lifecycle.quit.noActiveRuns"
    case quitRunContinues = "lifecycle.quit.run.continues"
    case quitRunStops = "lifecycle.quit.run.stops"
    case quitRunUnknown = "lifecycle.quit.run.unknown"
    case quitTitle = "lifecycle.quit.title"
    case runDetailBack = "runDetail.back"
    case runDetailCapability = "runDetail.capability"
    case runDetailEvent = "runDetail.event"
    case runDetailLoadMore = "runDetail.loadMore"
    case runDetailNoEvents = "runDetail.noEvents"
    case runDetailOpenInProvider = "runDetail.openInProvider"
    case runDetailPageUnavailable = "runDetail.pageUnavailable"
    case runDetailProviderHistory = "runDetail.providerHistory"
    case runDetailTitle = "runDetail.title"
    case runDetailUnavailable = "runDetail.unavailable"
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
        .dashboardChangesObservedDuringRun:
            "Observed during run: %llu files · +%llu −%llu",
        .dashboardChangesUnavailable: "Changes unavailable: %@",
        .dashboardDataUnavailable: "Dashboard data unavailable",
        .dashboardNoRuns: "No Runs",
        .dashboardSectionFinished: "Finished",
        .dashboardSectionNeedsAttention: "Needs Attention",
        .dashboardSectionPossiblyStuck: "Possibly Stuck",
        .dashboardSectionWorking: "Working",
        .dashboardUnavailable: "Dashboard unavailable",
        .dashboardViewActivity: "View activity",
        .closeToTrayAcknowledgement: "Keep Flit Running",
        .closeToTrayMessage:
            "Flit stays available in the menu bar so local monitoring can continue. Use Quit Flit to stop Flit monitoring and notifications.",
        .closeToTrayTitle: "Flit is still running",
        .boundaryChecking:
            "Verifying the local Core and Store. Provider monitoring has not started.",
        .boundaryReady:
            "The local Core and Store are ready. Provider monitoring has not started.",
        .boundaryUnavailable:
            "Flit could not open its local Core and Store safely. No agent controls are available.",
        .local: "Local by design",
        .menuBarTooltip: "Open Flit",
        .menuOpen: "Open Flit",
        .menuQuit: "Quit Flit",
        .quitActiveRuns: "Active provider Runs:",
        .quitCancel: "Cancel",
        .quitConfirm: "Quit Flit",
        .quitImpactUnavailable:
            "Flit could not verify active provider Runs. Their execution outcome after Quit is unknown.",
        .quitMonitoringBoundary:
            "Flit monitoring and notifications stop when you quit.",
        .quitNoActiveRuns: "No provider Runs are active.",
        .quitRunContinues: "• %@ — continues in %@",
        .quitRunStops: "• %@ — stops when Flit quits (%@)",
        .quitRunUnknown: "• %@ — outcome after Quit is unknown (%@)",
        .quitTitle: "Quit Flit?",
        .runDetailBack: "Back to Dashboard",
        .runDetailCapability: "%@: %@",
        .runDetailEvent: "%@ · %@ · %@ · %d%% confidence",
        .runDetailLoadMore: "Load more",
        .runDetailNoEvents: "No structured activity is available",
        .runDetailOpenInProvider: "Open in provider",
        .runDetailPageUnavailable: "More activity could not be loaded",
        .runDetailProviderHistory: "Provider history",
        .runDetailTitle: "Activity · %@",
        .runDetailUnavailable: "Run activity unavailable",
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
