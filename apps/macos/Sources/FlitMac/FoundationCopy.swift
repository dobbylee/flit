import Foundation

enum FoundationCopyKey: String {
    case attentionActionUnavailable = "attention.action.unavailable"
    case attentionCardEvidence = "attention.card.evidence"
    case attentionCardFacts = "attention.card.facts"
    case attentionCardTitle = "attention.card.title"
    case attentionCategoryCompletion = "attention.category.completion"
    case attentionCategoryFailure = "attention.category.failure"
    case attentionCategoryPermission = "attention.category.permission"
    case attentionCategoryPermissionAudit = "attention.category.permissionAudit"
    case attentionCategoryQuestion = "attention.category.question"
    case attentionCategoryRisk = "attention.category.risk"
    case attentionCategoryStuck = "attention.category.stuck"
    case attentionCategorySystem = "attention.category.system"
    case attentionContentUnavailable = "attention.content.unavailable"
    case attentionDetailsUnavailable = "attention.details.unavailable"
    case attentionPermissionDetailsRequired = "attention.permission.detailsRequired"
    case attentionPermissionAllowOnce = "attention.permission.allowOnce"
    case attentionPermissionDeny = "attention.permission.deny"
    case attentionSeverityActionRequired = "attention.severity.actionRequired"
    case attentionSeverityCritical = "attention.severity.critical"
    case attentionSeverityInformational = "attention.severity.informational"
    case attentionStatusDeliveryUnknown = "attention.status.deliveryUnknown"
    case attentionStatusOpen = "attention.status.open"
    case attentionStatusResponsePending = "attention.status.responsePending"
    case dashboardActivity = "dashboard.activity"
    case dashboardActivityUnknown = "dashboard.activity.unknown"
    case dashboardAttention = "dashboard.attention"
    case dashboardChanges = "dashboard.changes"
    case dashboardChangesObservedDuringRun = "dashboard.changes.observedDuringRun"
    case dashboardChangesUnavailable = "dashboard.changes.unavailable"
    case dashboardDataUnavailable = "dashboard.data.unavailable"
    case dashboardNoRuns = "dashboard.noRuns"
    case dashboardMonitoringUnavailable = "dashboard.monitoring.unavailable"
    case dashboardSectionFinished = "dashboard.section.finished"
    case dashboardSectionNeedsAttention = "dashboard.section.needsAttention"
    case dashboardSectionPossiblyStuck = "dashboard.section.possiblyStuck"
    case dashboardSectionWorking = "dashboard.section.working"
    case dashboardUnavailable = "dashboard.unavailable"
    case dashboardStillWorking = "dashboard.stillWorking"
    case dashboardStillWorkingApplied = "dashboard.stillWorking.applied"
    case dashboardStillWorkingRejected = "dashboard.stillWorking.rejected"
    case dashboardStillWorkingUnavailable = "dashboard.stillWorking.unavailable"
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
    case menuSettings = "lifecycle.menu.settings"
    case notificationSettingsCompletion = "notification.settings.completion"
    case notificationSettingsEffective = "notification.settings.effective"
    case notificationSettingsFailure = "notification.settings.failure"
    case notificationSettingsGlobal = "notification.settings.global"
    case notificationSettingsInherit = "notification.settings.inherit"
    case notificationSettingsInvalidTime = "notification.settings.invalidTime"
    case notificationSettingsLocalTime = "notification.settings.localTime"
    case notificationSettingsNoProjects = "notification.settings.noProjects"
    case notificationSettingsOff = "notification.settings.off"
    case notificationSettingsOn = "notification.settings.on"
    case notificationSettingsPermission = "notification.settings.permission"
    case notificationSettingsProject = "notification.settings.project"
    case notificationSettingsProjectMaster = "notification.settings.projectMaster"
    case notificationSettingsProjectPicker = "notification.settings.projectPicker"
    case notificationSettingsQuestion = "notification.settings.question"
    case notificationSettingsQuietEnd = "notification.settings.quietEnd"
    case notificationSettingsQuietHours = "notification.settings.quietHours"
    case notificationSettingsQuietStart = "notification.settings.quietStart"
    case notificationSettingsReload = "notification.settings.reload"
    case notificationSettingsSaveFailed = "notification.settings.saveFailed"
    case notificationSettingsSaveGlobal = "notification.settings.saveGlobal"
    case notificationSettingsSaveProject = "notification.settings.saveProject"
    case notificationSettingsStuck = "notification.settings.stuck"
    case notificationSettingsTitle = "notification.settings.title"
    case notificationSettingsUnavailable = "notification.settings.unavailable"
    case notificationCompletionBody = "notification.completion.body"
    case notificationFailureBody = "notification.failure.body"
    case notificationPermissionBody = "notification.permission.body"
    case notificationQuestionBody = "notification.question.body"
    case notificationStuckBody = "notification.stuck.body"
    case notificationStuckTitle = "notification.stuck.title"
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
    case runDetailEvidence = "runDetail.evidence"
    case runDetailEvidenceUnknown = "runDetail.evidence.unknown"
    case runDetailEvent = "runDetail.event"
    case runDetailFilter = "runDetail.filter"
    case runDetailFilterActivity = "runDetail.filter.activity"
    case runDetailFilterAll = "runDetail.filter.all"
    case runDetailFilterAttention = "runDetail.filter.attention"
    case runDetailFilterCommand = "runDetail.filter.command"
    case runDetailFilterFile = "runDetail.filter.file"
    case runDetailFilterLifecycle = "runDetail.filter.lifecycle"
    case runDetailFilterTest = "runDetail.filter.test"
    case runDetailGroup = "runDetail.group"
    case runDetailGroupLoadedThrough = "runDetail.group.loadedThrough"
    case runDetailHideEvidence = "runDetail.hideEvidence"
    case runDetailLoadMore = "runDetail.loadMore"
    case runDetailNoMatchingLoadedEvents = "runDetail.noMatchingLoadedEvents"
    case runDetailNoMatchingEvents = "runDetail.noMatchingEvents"
    case runDetailNoEvents = "runDetail.noEvents"
    case runDetailOpenInProvider = "runDetail.openInProvider"
    case runDetailOpenInProviderDegraded = "runDetail.openInProvider.degraded"
    case runDetailOpenInProviderSupported = "runDetail.openInProvider.supported"
    case runDetailOpenInProviderUnknown = "runDetail.openInProvider.unknown"
    case runDetailOpenInProviderUnavailable = "runDetail.openInProvider.unavailable"
    case runDetailOpenInProviderUnsupported = "runDetail.openInProvider.unsupported"
    case runDetailPageUnavailable = "runDetail.pageUnavailable"
    case runDetailProviderHistory = "runDetail.providerHistory"
    case runDetailRawPayloadUnavailable = "runDetail.rawPayloadUnavailable"
    case runDetailSummaryBranchUnavailable = "runDetail.summary.branchUnavailable"
    case runDetailSummaryEvidenceUnavailable = "runDetail.summary.evidenceUnavailable"
    case runDetailSummaryOpenIssuesUnavailable = "runDetail.summary.openIssuesUnavailable"
    case runDetailSummaryProjectProvider = "runDetail.summary.projectProvider"
    case runDetailSummaryResult = "runDetail.summary.result"
    case runDetailSummaryStartUnavailable = "runDetail.summary.startUnavailable"
    case runDetailSummaryTime = "runDetail.summary.time"
    case runDetailSummaryValidationUnavailable = "runDetail.summary.validationUnavailable"
    case runDetailCompletionSummary = "runDetail.completionSummary"
    case runDetailShowEvidence = "runDetail.showEvidence"
    case runDetailTitle = "runDetail.title"
    case runDetailUnavailable = "runDetail.unavailable"
    case runChangesAttributionExact = "runChanges.attribution.exact"
    case runChangesAttributionObserved = "runChanges.attribution.observed"
    case runChangesBaselineHead = "runChanges.baselineHead"
    case runChangesBinary = "runChanges.binary"
    case runChangesCommitted = "runChanges.committed"
    case runChangesChangeSetNotAvailable = "runChanges.changeSetNotAvailable"
    case runChangesFirstPageUnavailable = "runChanges.firstPageUnavailable"
    case runChangesHeadUnavailable = "runChanges.headUnavailable"
    case runChangesLayers = "runChanges.layers"
    case runChangesLineCounts = "runChanges.lineCounts"
    case runChangesLineCountsUnavailable = "runChanges.lineCountsUnavailable"
    case runChangesLoadMore = "runChanges.loadMore"
    case runChangesNoChanges = "runChanges.noChanges"
    case runChangesOpenChangeNotFound = "runChanges.open.changeNotFound"
    case runChangesOpenChangeSetUnavailable = "runChanges.open.changeSetUnavailable"
    case runChangesOpenDeleted = "runChanges.open.deleted"
    case runChangesOpenExternally = "runChanges.open.externally"
    case runChangesOpenFailed = "runChanges.open.failed"
    case runChangesOpenHandlerFailed = "runChanges.open.handlerFailed"
    case runChangesOpenedExternally = "runChanges.openedExternally"
    case runChangesOpenOutsideProject = "runChanges.open.outsideProject"
    case runChangesOpenProjectChanged = "runChanges.open.projectChanged"
    case runChangesOpenRepositoryChanged = "runChanges.open.repositoryChanged"
    case runChangesOpenSymlinkEscape = "runChanges.open.symlinkEscape"
    case runChangesOpenTargetChanged = "runChanges.open.targetChanged"
    case runChangesOpenTargetNotFile = "runChanges.open.targetNotFile"
    case runChangesOpenTargetUnavailable = "runChanges.open.targetUnavailable"
    case runChangesOpenUnavailable = "runChanges.open.unavailable"
    case runChangesPageUnavailable = "runChanges.pageUnavailable"
    case runChangesScopeInside = "runChanges.scope.inside"
    case runChangesScopeOutside = "runChanges.scope.outside"
    case runChangesStaged = "runChanges.staged"
    case runChangesStatusAdded = "runChanges.status.added"
    case runChangesStatusDeleted = "runChanges.status.deleted"
    case runChangesStatusModified = "runChanges.status.modified"
    case runChangesStatusTypeChanged = "runChanges.status.typeChanged"
    case runChangesStatusUntracked = "runChanges.status.untracked"
    case runChangesTerminalHead = "runChanges.terminalHead"
    case runChangesText = "runChanges.text"
    case runChangesTitle = "runChanges.title"
    case runChangesUnavailable = "runChanges.unavailable"
    case runChangesUnstaged = "runChanges.unstaged"
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
        .attentionActionUnavailable: "No action is available for this attention",
        .attentionCardEvidence: "Evidence: %@ · %@",
        .attentionCardFacts: "%@ · %@ · %@",
        .attentionCardTitle: "Highest-priority attention",
        .attentionCategoryCompletion: "Completion",
        .attentionCategoryFailure: "Failure",
        .attentionCategoryPermission: "Permission",
        .attentionCategoryPermissionAudit: "Permission audit",
        .attentionCategoryQuestion: "Question",
        .attentionCategoryRisk: "Risk",
        .attentionCategoryStuck: "Possibly stuck",
        .attentionCategorySystem: "System",
        .attentionContentUnavailable: "Details unavailable: provider content was not retained",
        .attentionDetailsUnavailable: "Attention details unavailable",
        .attentionPermissionDetailsRequired:
            "Response unavailable until the command, cwd, affected paths, and provider request text can be shown",
        .attentionPermissionAllowOnce: "Allow once",
        .attentionPermissionDeny: "Deny",
        .attentionSeverityActionRequired: "Action required",
        .attentionSeverityCritical: "Critical",
        .attentionSeverityInformational: "Informational",
        .attentionStatusDeliveryUnknown: "Delivery could not be confirmed",
        .attentionStatusOpen: "Open",
        .attentionStatusResponsePending: "Sending response",
        .dashboardActivity: "Activity: %@ · %d%% confidence",
        .dashboardActivityUnknown: "Activity: Unknown",
        .dashboardAttention: "Attention: %@ · %llu open",
        .dashboardChanges: "Changes: %llu files · +%llu −%llu",
        .dashboardChangesObservedDuringRun:
            "Observed during run: %llu files · +%llu −%llu",
        .dashboardChangesUnavailable: "Changes unavailable: %@",
        .dashboardDataUnavailable: "Dashboard data unavailable",
        .dashboardNoRuns: "No Runs",
        .dashboardMonitoringUnavailable:
            "Dashboard monitoring could not converge. Showing the last accepted state.",
        .dashboardSectionFinished: "Finished",
        .dashboardSectionNeedsAttention: "Needs Attention",
        .dashboardSectionPossiblyStuck: "Possibly Stuck",
        .dashboardSectionWorking: "Working",
        .dashboardUnavailable: "Dashboard unavailable",
        .dashboardStillWorking: "Still working",
        .dashboardStillWorkingApplied: "Still working was recorded",
        .dashboardStillWorkingRejected: "Still working was not applied: %@",
        .dashboardStillWorkingUnavailable: "Still working could not be completed",
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
        .menuSettings: "Settings…",
        .notificationSettingsCompletion: "Finished",
        .notificationSettingsEffective: "Currently enabled: %@",
        .notificationSettingsFailure: "Failed or interrupted",
        .notificationSettingsGlobal: "Global notifications",
        .notificationSettingsInherit: "Inherit",
        .notificationSettingsInvalidTime:
            "Enter two different local times in 24-hour HH:MM format.",
        .notificationSettingsLocalTime:
            "Uses this Mac's current local time. Overnight ranges are supported.",
        .notificationSettingsNoProjects: "No active Projects are available.",
        .notificationSettingsOff: "Off",
        .notificationSettingsOn: "On",
        .notificationSettingsPermission: "Permission requests",
        .notificationSettingsProject: "Project overrides",
        .notificationSettingsProjectMaster: "Project notifications",
        .notificationSettingsProjectPicker: "Project",
        .notificationSettingsQuestion: "Questions",
        .notificationSettingsQuietEnd: "End (HH:MM)",
        .notificationSettingsQuietHours: "Quiet hours",
        .notificationSettingsQuietStart: "Start (HH:MM)",
        .notificationSettingsReload: "Reload",
        .notificationSettingsSaveFailed:
            "Settings were not saved. Reload the authoritative policy and try again.",
        .notificationSettingsSaveGlobal: "Save global settings",
        .notificationSettingsSaveProject: "Save Project overrides",
        .notificationSettingsStuck: "Possibly stuck",
        .notificationSettingsTitle: "Notification Settings",
        .notificationSettingsUnavailable:
            "Notification settings are unavailable. The last accepted values were preserved.",
        .notificationCompletionBody: "%@ · Run completed",
        .notificationFailureBody: "%@ · Run failed",
        .notificationPermissionBody: "%@ · Permission required",
        .notificationQuestionBody: "%@ · Question waiting",
        .notificationStuckBody: "%@ · Possibly stuck",
        .notificationStuckTitle: "Flit needs your attention",
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
        .runDetailEvidence: "Evidence %@ · %@ · %@ · %@ · %d%% · captured %@",
        .runDetailEvidenceUnknown: "Unknown",
        .runDetailEvent: "%@ · %@ · %@ · %d%% confidence",
        .runDetailFilter: "Filter",
        .runDetailFilterActivity: "Activity",
        .runDetailFilterAll: "All",
        .runDetailFilterAttention: "Attention",
        .runDetailFilterCommand: "Command",
        .runDetailFilterFile: "File",
        .runDetailFilterLifecycle: "Lifecycle",
        .runDetailFilterTest: "Test",
        .runDetailGroup: "%@ – %@ · %@ · %d events",
        .runDetailGroupLoadedThrough: "Started %@ · loaded through %@ · %@ · %d events",
        .runDetailHideEvidence: "Hide evidence",
        .runDetailLoadMore: "Load more",
        .runDetailNoMatchingLoadedEvents: "No %@ events are loaded yet",
        .runDetailNoMatchingEvents: "No %@ events are available",
        .runDetailNoEvents: "No structured activity is available",
        .runDetailOpenInProvider: "Open in provider",
        .runDetailOpenInProviderDegraded:
            "Open in provider is disabled because the capability is degraded",
        .runDetailOpenInProviderSupported:
            "Open in provider is disabled because this Flit build has no guarded open implementation",
        .runDetailOpenInProviderUnknown:
            "Open in provider is disabled because the capability is unknown",
        .runDetailOpenInProviderUnavailable:
            "Open in provider is disabled because the capability is unavailable",
        .runDetailOpenInProviderUnsupported:
            "Open in provider is disabled because this provider documents no open action",
        .runDetailPageUnavailable: "More activity could not be loaded",
        .runDetailProviderHistory: "Provider history",
        .runDetailRawPayloadUnavailable:
            "Raw payload unavailable in this bounded activity view",
        .runDetailSummaryBranchUnavailable:
            "Branch unavailable in the current Dashboard projection",
        .runDetailSummaryEvidenceUnavailable:
            "Completion evidence unavailable in the current Dashboard projection",
        .runDetailSummaryOpenIssuesUnavailable:
            "Open issues unavailable in the current Dashboard projection",
        .runDetailSummaryProjectProvider: "Project: %@ · Provider: %@",
        .runDetailSummaryResult: "Result: %@",
        .runDetailSummaryStartUnavailable: "Started: unavailable · Ended: %@",
        .runDetailSummaryTime: "Started: %@ · Ended: %@",
        .runDetailSummaryValidationUnavailable:
            "Validation unavailable in the current Dashboard projection",
        .runDetailCompletionSummary: "Completion summary",
        .runDetailShowEvidence: "Show evidence",
        .runDetailTitle: "Activity · %@",
        .runDetailUnavailable: "Run activity unavailable",
        .runChangesAttributionExact: "Attribution: Exact",
        .runChangesAttributionObserved: "Attribution: Observed during run",
        .runChangesBaselineHead: "Baseline HEAD: %@",
        .runChangesBinary: "Binary",
        .runChangesCommitted: "Committed",
        .runChangesChangeSetNotAvailable: "Terminal change set not available",
        .runChangesFirstPageUnavailable: "Changes could not be loaded",
        .runChangesHeadUnavailable: "Unavailable",
        .runChangesLayers: "Layers: %@",
        .runChangesLineCounts: "+%llu −%llu",
        .runChangesLineCountsUnavailable: "Line counts unavailable",
        .runChangesLoadMore: "Load more changes",
        .runChangesNoChanges: "No file changes are available",
        .runChangesOpenChangeNotFound: "The stored change no longer exists",
        .runChangesOpenChangeSetUnavailable: "The terminal change set is unavailable",
        .runChangesOpenDeleted: "Deleted files cannot be opened",
        .runChangesOpenExternally: "Open externally",
        .runChangesOpenFailed: "The external-open request could not be completed",
        .runChangesOpenHandlerFailed: "The default application could not open the file",
        .runChangesOpenedExternally: "Opened with the default application",
        .runChangesOpenOutsideProject: "Files outside the Project cannot be opened",
        .runChangesOpenProjectChanged: "The Project identity changed",
        .runChangesOpenRepositoryChanged: "The repository identity changed",
        .runChangesOpenSymlinkEscape: "The current file target escapes its stored boundary",
        .runChangesOpenTargetChanged: "The file identity changed during validation",
        .runChangesOpenTargetNotFile: "The current target is not a file",
        .runChangesOpenTargetUnavailable: "The current file target is unavailable",
        .runChangesOpenUnavailable: "Open unavailable: %@",
        .runChangesPageUnavailable: "More changes could not be loaded",
        .runChangesScopeInside: "Inside Project",
        .runChangesScopeOutside: "Outside Project",
        .runChangesStaged: "Staged",
        .runChangesStatusAdded: "Added",
        .runChangesStatusDeleted: "Deleted",
        .runChangesStatusModified: "Modified",
        .runChangesStatusTypeChanged: "Type changed",
        .runChangesStatusUntracked: "Untracked",
        .runChangesTerminalHead: "Terminal HEAD: %@",
        .runChangesText: "Text",
        .runChangesTitle: "Changes",
        .runChangesUnavailable: "Changes unavailable: %@",
        .runChangesUnstaged: "Unstaged",
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

    static func providerOpenUnavailableReason(_ status: FlitCapabilityStatus) -> String {
        let key: FoundationCopyKey
        switch status {
        case .supported: key = .runDetailOpenInProviderSupported
        case .degraded: key = .runDetailOpenInProviderDegraded
        case .unsupported: key = .runDetailOpenInProviderUnsupported
        case .unknown: key = .runDetailOpenInProviderUnknown
        case .unavailable: key = .runDetailOpenInProviderUnavailable
        }
        return text(key)
    }

    static func attentionCategory(_ category: FlitRunActiveAttentionCategory) -> String {
        let key: FoundationCopyKey
        switch category {
        case .permission: key = .attentionCategoryPermission
        case .permissionAudit: key = .attentionCategoryPermissionAudit
        case .question: key = .attentionCategoryQuestion
        case .risk: key = .attentionCategoryRisk
        case .failure: key = .attentionCategoryFailure
        case .stuck: key = .attentionCategoryStuck
        case .system: key = .attentionCategorySystem
        case .completion: key = .attentionCategoryCompletion
        }
        return text(key)
    }

    static func attentionSeverity(_ severity: FlitRunActiveAttentionSeverity) -> String {
        let key: FoundationCopyKey
        switch severity {
        case .informational: key = .attentionSeverityInformational
        case .actionRequired: key = .attentionSeverityActionRequired
        case .critical: key = .attentionSeverityCritical
        }
        return text(key)
    }

    static func attentionStatus(_ status: FlitRunActiveAttentionStatus) -> String {
        let key: FoundationCopyKey
        switch status {
        case .open: key = .attentionStatusOpen
        case .responsePending: key = .attentionStatusResponsePending
        case .deliveryUnknown: key = .attentionStatusDeliveryUnknown
        }
        return text(key)
    }
}
