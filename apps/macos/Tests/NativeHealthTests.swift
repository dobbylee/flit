import AppKit
import Foundation

enum NativeHealthTestFailure: Error {
    case failed(String)
}

@MainActor
private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    if !condition() {
        throw NativeHealthTestFailure.failed(message)
    }
}

private func canonicalJSON(at path: String) throws -> Data {
    let object = try JSONSerialization.jsonObject(
        with: Data(contentsOf: URL(fileURLWithPath: path))
    )
    return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
}

private func canonicalJSON(from text: String) throws -> Data {
    let object = try JSONSerialization.jsonObject(with: Data(text.utf8))
    return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
}

private func decodeFixture<T: Decodable>(_ type: T.Type, at path: String) throws -> T {
    try JSONDecoder().decode(type, from: Data(contentsOf: URL(fileURLWithPath: path)))
}

private func decodeDashboardFixture(
    at path: String,
    replacing field: String,
    with value: String
) throws -> FlitDashboardReadResponse {
    let data = try Data(contentsOf: URL(fileURLWithPath: path))
    guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw NativeHealthTestFailure.failed("Dashboard fixture must be an object")
    }
    object[field] = value
    return try JSONDecoder().decode(
        FlitDashboardReadResponse.self,
        from: try JSONSerialization.data(withJSONObject: object)
    )
}

@MainActor
private func descendants(of view: NSView) -> [NSView] {
    view.subviews.flatMap { [$0] + descendants(of: $0) }
}

@MainActor
private func requireDecodingFailure<T: Decodable>(
    _ type: T.Type,
    from data: Data,
    _ message: String
) throws {
    do {
        _ = try JSONDecoder().decode(type, from: data)
    } catch {
        return
    }
    throw NativeHealthTestFailure.failed(message)
}

@MainActor
private final class RecordingCloseToTrayAlertPresenter: CloseToTrayAlertPresenting {
    private(set) var contents: [CloseToTrayAlertContent] = []
    private var completion: (() -> Void)?

    func present(
        _ content: CloseToTrayAlertContent,
        for window: NSWindow,
        completion: @escaping () -> Void
    ) {
        contents.append(content)
        self.completion = completion
    }

    func acknowledge() {
        let pending = completion
        completion = nil
        pending?()
    }
}

@MainActor
private final class RecordingExplicitQuitAlertPresenter: ExplicitQuitAlertPresenting {
    private(set) var contents: [ExplicitQuitAlertContent] = []
    private var completions: [(ExplicitQuitChoice) -> Void] = []

    func present(
        _ content: ExplicitQuitAlertContent,
        for window: NSWindow,
        completion: @escaping (ExplicitQuitChoice) -> Void
    ) {
        contents.append(content)
        completions.append(completion)
    }

    func choose(_ choice: ExplicitQuitChoice) {
        guard !completions.isEmpty else { return }
        completions.removeFirst()(choice)
    }
}

private func changingQuitImpact(
    _ response: FlitQuitImpactResponse,
    cursor: UInt64
) throws -> FlitQuitImpactResponse {
    let data = try JSONEncoder().encode(response)
    guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw NativeHealthTestFailure.failed("Quit impact response must be an object")
    }
    object["cursor"] = cursor
    return try JSONDecoder().decode(
        FlitQuitImpactResponse.self,
        from: try JSONSerialization.data(withJSONObject: object)
    )
}

private func changingRunDetail(
    _ response: FlitRunDetailReadResponse,
    mutate: (inout [String: Any]) throws -> Void
) throws -> FlitRunDetailReadResponse {
    let data = try JSONEncoder().encode(response)
    guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw NativeHealthTestFailure.failed("Run detail response must be an object")
    }
    try mutate(&object)
    return try JSONDecoder().decode(
        FlitRunDetailReadResponse.self,
        from: try JSONSerialization.data(withJSONObject: object)
    )
}

private func runEvidenceObject(
    cursor: UInt64,
    eventId: String,
    sessionId: String?,
    eventType: String,
    category: FlitRunEvidenceCategory,
    sourceKind: FlitEventSourceKind,
    confidence: Double,
    observedAt: String
) throws -> [String: Any] {
    let record = FlitRunEvidenceRecord(
        cursor: cursor,
        eventId: eventId,
        sessionId: sessionId,
        eventType: eventType,
        category: category,
        sourceKind: sourceKind,
        confidence: confidence,
        observedAt: observedAt
    )
    let data = try JSONEncoder().encode(record)
    guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw NativeHealthTestFailure.failed("Run evidence record must be an object")
    }
    if sessionId == nil {
        object["session_id"] = NSNull()
    }
    return object
}

private func changingDashboard(
    _ response: FlitDashboardReadResponse,
    mutate: (inout [String: Any]) throws -> Void
) throws -> FlitDashboardReadResponse {
    let data = try JSONEncoder().encode(response)
    guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw NativeHealthTestFailure.failed("Dashboard response must be an object")
    }
    try mutate(&object)
    return try JSONDecoder().decode(
        FlitDashboardReadResponse.self,
        from: try JSONSerialization.data(withJSONObject: object)
    )
}

@MainActor
private func requireCommandError(
    _ text: String,
    code: FlitCommandErrorCode,
    fixtures: [FlitCommandError]
) throws {
    let actual = try JSONDecoder().decode(FlitCommandError.self, from: Data(text.utf8))
    guard let expected = fixtures.first(where: { $0.code == code }) else {
        throw NativeHealthTestFailure.failed("missing fixture for command error \(code)")
    }
    try require(actual == expected, "real command error \(code) must match its fixture")
}

@MainActor
private func requireFoundationLayout(
    _ controller: FoundationViewController,
    in window: NSWindow,
    size: NSSize,
    expectedPanelWidth: CGFloat
) throws {
    window.setContentSize(size)
    window.contentView?.needsLayout = true
    window.contentView?.layoutSubtreeIfNeeded()
    guard let panelFrame = controller.foundationPanelFrame else {
        throw NativeHealthTestFailure.failed("foundation panel must be available after layout")
    }
    try require(
        !controller.hasAmbiguousFoundationLayout,
        "foundation layout must not be ambiguous at width \(size.width)"
    )
    try require(
        abs(panelFrame.width - expectedPanelWidth) < 0.5,
        "foundation panel width \(panelFrame.width) must be \(expectedPanelWidth) at requested width \(size.width), actual width \(controller.view.bounds.width)"
    )
    try require(
        panelFrame.minX >= 48 && panelFrame.maxX <= size.width - 48,
        "foundation panel must preserve 48-point side margins"
    )
    try require(
        panelFrame.minY >= 0 && panelFrame.maxY <= controller.view.bounds.maxY,
        "foundation panel must remain reachable at height \(size.height)"
    )
}

@main
@MainActor
struct NativeHealthTests {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else {
            throw NativeHealthTestFailure.failed("expected repository root argument")
        }
        let root = CommandLine.arguments[1]
        let fixtureRoot =
            "\(root)/fixtures/protocol/commands/v\(flitClientProtocolVersion)"
        let dataDirectory = "\(root)/target/flit-macos/native-health-data"

        let requestData = try canonicalJSON(
            at: "\(fixtureRoot)/system_health.request.json"
        )
        guard
            let requestObject = try JSONSerialization.jsonObject(with: requestData)
                as? [String: String],
            let requestVersion = requestObject["client_protocol_version"]
        else {
            throw NativeHealthTestFailure.failed("health request fixture must be a string map")
        }

        let client = SystemHealthClient()
        let retainedV11BridgeError: BridgeError = .ProjectResponseTooLarge
        try require(
            retainedV11BridgeError == .ProjectResponseTooLarge,
            "current protocol must retain the generated Project response error case"
        )
        let managedRunBridgeError: BridgeError = .ManagedRunResponseTooLarge
        try require(
            managedRunBridgeError == .ManagedRunResponseTooLarge,
            "current protocol must generate the managed Run bridge error case"
        )
        let dashboardBridgeError: BridgeError = .DashboardResponseTooLarge
        try require(
            dashboardBridgeError == .DashboardResponseTooLarge,
            "current protocol must generate the bounded Dashboard response error case"
        )
        let quitImpactBridgeError: BridgeError = .QuitImpactResponseTooLarge
        try require(
            quitImpactBridgeError == .QuitImpactResponseTooLarge,
            "current protocol must generate the bounded Quit impact response error case"
        )
        let outgoingRequest = try JSONSerialization.data(
            withJSONObject: ["client_protocol_version": client.clientProtocolVersion],
            options: [.sortedKeys]
        )
        try require(
            outgoingRequest == requestData,
            "native client request must match the repository fixture"
        )
        let providerReadyHealth = try String(
            contentsOfFile: "\(fixtureRoot)/system_health.providers_ready.response.json",
            encoding: .utf8
        )
        let providerUnavailableHealth = try String(
            contentsOfFile: "\(fixtureRoot)/system_health.providers_unavailable.response.json",
            encoding: .utf8
        )
        guard case .ready = client.classify(providerReadyHealth) else {
            throw NativeHealthTestFailure.failed(
                "native health must remain ready after a supported provider probe"
            )
        }
        try require(
            client.classify(providerUnavailableHealth) == .unavailable(messageKey: nil),
            "native health must become unavailable after a failed or unknown provider probe"
        )

        let projectErrors = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/project_errors.json"
        )
        let protocolMismatchError = try decodeFixture(
            FlitCommandError.self,
            at: "\(fixtureRoot)/protocol_mismatch.error.json"
        )
        let commandErrors = projectErrors + [protocolMismatchError]
        try require(
            Set(commandErrors.map { $0.code.rawValue }).count == 8,
            "generated command errors must decode every Project command error code"
        )
        try requireCommandError(
            try projectsListPageJson(
                afterDisplayName: nil,
                afterProjectId: nil,
                limit: 1,
                clientProtocolVersion: requestVersion
            ),
            code: .storageUnavailable,
            fixtures: commandErrors
        )
        try requireCommandError(
            try providerDiagnosticsJson(clientProtocolVersion: requestVersion),
            code: .storageUnavailable,
            fixtures: commandErrors
        )
        let dashboardErrors = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/dashboard_read_errors.json"
        )
        let initialDashboardRequest = try String(
            contentsOfFile: "\(fixtureRoot)/dashboard_read.initial.request.json",
            encoding: .utf8
        )
        try requireCommandError(
            try dashboardReadJson(requestJson: initialDashboardRequest),
            code: .storageUnavailable,
            fixtures: dashboardErrors
        )
        try requireCommandError(
            try quitImpactJson(clientProtocolVersion: requestVersion),
            code: .storageUnavailable,
            fixtures: commandErrors
        )
        try requireCommandError(
            try projectInspectJson(
                selectedPath: "\(root)/target/flit-macos/mismatch-must-not-be-read",
                clientProtocolVersion: "2.0"
            ),
            code: .protocolMismatch,
            fixtures: commandErrors
        )

        try initializeCore(
            dataDirectory: dataDirectory,
            clientProtocolVersion: requestVersion
        )
        guard case .ready = client.load() else {
            throw NativeHealthTestFailure.failed(
                "native client must observe validated storage"
            )
        }
        let normal = try systemHealthJson(clientProtocolVersion: requestVersion)
        let mismatch = try systemHealthJson(clientProtocolVersion: "2.0")
        let expectedNormal = try canonicalJSON(
            at: "\(fixtureRoot)/system_health.response.json"
        )
        let expectedMismatch = try canonicalJSON(
            at: "\(fixtureRoot)/protocol_mismatch.error.json"
        )
        let actualNormal = try canonicalJSON(from: normal)
        let actualMismatch = try canonicalJSON(from: mismatch)
        try require(
            actualNormal == expectedNormal,
            "normal health payload must match the repository fixture"
        )
        try require(
            actualMismatch == expectedMismatch,
            "protocol mismatch payload must match the repository fixture"
        )
        try require(coreConstructionCount() == 1, "bridge calls must share one Core construction")
        let initialDashboard = try JSONDecoder().decode(
            FlitDashboardReadResponse.self,
            from: Data(try dashboardReadJson(requestJson: initialDashboardRequest).utf8)
        )
        let dashboardCoreInstanceId: String
        let dashboardCursor: UInt64
        switch initialDashboard {
        case let .snapshot(response):
            try require(
                response.delivery == .snapshot
                    && response.reason == .initial
                    && response.runs.isEmpty
                    && !response.hasMore,
                "native initial Dashboard read must return one empty persisted snapshot"
            )
            dashboardCoreInstanceId = response.coreInstanceId
            dashboardCursor = response.nextCursor
        case .delta:
            throw NativeHealthTestFailure.failed(
                "native initial Dashboard read must not return a delta"
            )
        }
        let currentDashboardRequest = FlitDashboardReadRequest(
            expectedCoreInstanceId: dashboardCoreInstanceId,
            afterCursor: dashboardCursor,
            requestedEventLimit: 50,
            clientProtocolVersion: requestVersion
        )
        let currentDashboard = try JSONDecoder().decode(
            FlitDashboardReadResponse.self,
            from: Data(
                try dashboardReadJson(
                    requestJson: String(
                        data: try JSONEncoder().encode(currentDashboardRequest),
                        encoding: .utf8
                    )!
                ).utf8
            )
        )
        guard case let .delta(currentDelta) = currentDashboard else {
            throw NativeHealthTestFailure.failed(
                "native current Dashboard cursor must return a delta"
            )
        }
        try require(
            currentDelta.delivery == .delta
                && currentDelta.events.isEmpty
                && currentDelta.runs.isEmpty
                && currentDelta.nextCursor == dashboardCursor
                && !currentDelta.hasMore,
            "native current Dashboard delta must converge without a callback stream"
        )
        let emptyQuitImpact = try JSONDecoder().decode(
            FlitQuitImpactResponse.self,
            from: Data(try quitImpactJson(clientProtocolVersion: requestVersion).utf8)
        )
        try require(
            emptyQuitImpact.runs.isEmpty
                && emptyQuitImpact.flitMonitoringStops
                && emptyQuitImpact.flitNotificationsStop
                && emptyQuitImpact.coreInstanceId == dashboardCoreInstanceId
                && emptyQuitImpact.cursor == dashboardCursor,
            "native Quit impact must preserve the exact empty Core snapshot"
        )
        let missingDetailRequest = FlitRunDetailReadRequest(
            runId: "run-missing",
            expectedRunVersion: 1,
            afterCursor: 0,
            requestedEventLimit: 50,
            clientProtocolVersion: requestVersion
        )
        let missingDetailError = try JSONDecoder().decode(
            FlitCommandError.self,
            from: Data(
                try runDetailReadJson(
                    requestJson: String(
                        data: try JSONEncoder().encode(missingDetailRequest),
                        encoding: .utf8
                    )!
                ).utf8
            )
        )
        try require(
            missingDetailError.code == .runNotFound,
            "native Run detail must return a typed missing-Run boundary"
        )

        let inspectionFixture = try decodeFixture(
            FlitProjectInspectionResponse.self,
            at: "\(fixtureRoot)/project_inspect.response.json"
        )
        try require(
            inspectionFixture.protocolVersion == flitClientProtocolVersion,
            "generated inspection response must decode the current protocol fixture"
        )
        _ = try decodeFixture(
            FlitProjectInspectionRequest.self,
            at: "\(fixtureRoot)/project_inspect.request.json"
        )
        _ = try decodeFixture(
            FlitProjectRegistrationRequest.self,
            at: "\(fixtureRoot)/project_register.request.json"
        )
        for (name, expectedStatus) in [
            (
                "project_register.registered.response.json",
                FlitProjectRegistrationStatus.registered
            ),
            (
                "project_register.duplicate_canonical_path.response.json",
                FlitProjectRegistrationStatus.duplicateCanonicalPath
            ),
            (
                "project_register.duplicate_filesystem_identity.response.json",
                FlitProjectRegistrationStatus.duplicateFilesystemIdentity
            ),
        ] {
            let response = try decodeFixture(
                FlitProjectRegistrationResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                response.status == expectedStatus,
                "generated registration response must decode \(expectedStatus)"
            )
        }
        _ = try decodeFixture(
            FlitProjectTrustRequest.self,
            at: "\(fixtureRoot)/project_trust.request.json"
        )
        for (name, expectedStatus) in [
            (
                "project_trust.trusted.response.json",
                FlitProjectTrustStatus.trusted
            ),
            (
                "project_trust.already_trusted.response.json",
                FlitProjectTrustStatus.alreadyTrusted
            ),
        ] {
            let response = try decodeFixture(
                FlitProjectTrustResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                response.status == expectedStatus,
                "generated trust response must decode \(expectedStatus)"
            )
        }
        _ = try decodeFixture(
            FlitProjectsListRequest.self,
            at: "\(fixtureRoot)/projects_list.request.json"
        )
        _ = try decodeFixture(
            FlitProjectsListResponse.self,
            at: "\(fixtureRoot)/projects_list.response.json"
        )
        _ = try decodeFixture(
            FlitDashboardReadRequest.self,
            at: "\(fixtureRoot)/dashboard_read.initial.request.json"
        )
        _ = try decodeFixture(
            FlitDashboardReadRequest.self,
            at: "\(fixtureRoot)/dashboard_read.delta.request.json"
        )
        let initialDashboardFixture = try decodeFixture(
            FlitDashboardReadResponse.self,
            at: "\(fixtureRoot)/dashboard_read.initial.response.json"
        )
        let deltaDashboardFixture = try decodeFixture(
            FlitDashboardReadResponse.self,
            at: "\(fixtureRoot)/dashboard_read.delta.response.json"
        )
        let resyncDashboardFixture = try decodeFixture(
            FlitDashboardReadResponse.self,
            at: "\(fixtureRoot)/dashboard_read.resync.response.json"
        )
        let unavailableChangesDashboardFixture = try decodeFixture(
            FlitDashboardReadResponse.self,
            at: "\(fixtureRoot)/dashboard_read.unavailable_changes.response.json"
        )
        let overflowFixtureData = try Data(
            contentsOf: URL(
                fileURLWithPath: "\(fixtureRoot)/dashboard_read.initial.response.json"
            )
        )
        guard
            var overflowFixtureObject = try JSONSerialization.jsonObject(
                with: overflowFixtureData
            ) as? [String: Any],
            let overflowBaseRun = (overflowFixtureObject["runs"] as? [[String: Any]])?.first
        else {
            throw NativeHealthTestFailure.failed(
                "Dashboard overflow fixture must contain one Run"
            )
        }
        var observedFixtureObject = overflowFixtureObject
        var observedRuns = observedFixtureObject["runs"] as? [[String: Any]] ?? []
        var observedRun = observedRuns[0]
        var observedChanges = observedRun["changes"] as? [String: Any] ?? [:]
        observedChanges["attribution"] = "observed_during_run"
        observedRun["changes"] = observedChanges
        observedRuns[0] = observedRun
        observedFixtureObject["runs"] = observedRuns
        let observedDashboardFixture = try JSONDecoder().decode(
            FlitDashboardReadResponse.self,
            from: try JSONSerialization.data(withJSONObject: observedFixtureObject)
        )
        let overflowFirstRunId = "overflow-needs-attention-0"
        overflowFixtureObject["runs"] = (0..<8).map { index -> [String: Any] in
            var run = overflowBaseRun
            let needsAttention = index < 4
            run["run_id"] = needsAttention
                ? "overflow-needs-attention-\(index)"
                : "overflow-finished-\(index)"
            run["dashboard_bucket"] = needsAttention ? "NeedsAttention" : "Finished"
            run["title"] = needsAttention
                ? "Needs Attention \(index)"
                : "Finished \(index)"
            return run
        }
        let overflowDashboardFixture = try JSONDecoder().decode(
            FlitDashboardReadResponse.self,
            from: try JSONSerialization.data(withJSONObject: overflowFixtureObject)
        )
        guard
            case let .snapshot(initialSnapshotFixture) = initialDashboardFixture,
            case let .delta(deltaFixture) = deltaDashboardFixture,
            case let .snapshot(resyncFixture) = resyncDashboardFixture,
            case let .snapshot(unavailableSnapshotFixture) =
                unavailableChangesDashboardFixture,
            case let .available(attribution, files, insertions, deletions) =
                initialSnapshotFixture.runs[0].changes,
            case let .unavailable(unavailableReason) =
                unavailableSnapshotFixture.runs[0].changes
        else {
            throw NativeHealthTestFailure.failed(
                "generated Dashboard fixtures must preserve tagged delivery variants"
            )
        }
        try require(
            initialSnapshotFixture.reason == .initial
                && initialSnapshotFixture.runs.count == 1
                && initialSnapshotFixture.runs[0].attentionOpenCount == 2
                && attribution == .exact
                && files == 3
                && insertions == 42
                && deletions == 7
                && unavailableReason == "git_observation_not_configured"
                && deltaFixture.events.count == 1
                && deltaFixture.runs.count == 1
                && deltaFixture.runs[0].runId == deltaFixture.events[0].runId
                && deltaFixture.runs[0].version == deltaFixture.nextCursor
                && resyncFixture.reason == .coreInstanceMismatch,
            "generated Dashboard fixtures must preserve snapshot, delta, and resync facts"
        )
        try require(
            DashboardSection.allCases.map(\.rawValue)
                == ["NeedsAttention", "PossiblyStuck", "Working", "Finished"],
            "native Dashboard section order must match the Core-owned buckets"
        )
        var presentation = DashboardPresentationState()
        try presentation.apply(initialDashboardFixture)
        let initialWorkingRuns = try presentation.runs(in: .working)
        try require(
            initialWorkingRuns.count == 1
                && presentation.cursor == initialSnapshotFixture.nextCursor,
            "native snapshot must replace presentation state with exact Core records"
        )
        try presentation.apply(deltaDashboardFixture)
        let deltaWorkingRuns = try presentation.runs(in: .working)
        try require(
            deltaWorkingRuns[0].version == deltaFixture.nextCursor
                && presentation.cursor == deltaFixture.nextCursor,
            "native delta must upsert the supplied Core projection without reducing locators"
        )
        for (fixtureName, field) in [
            ("dashboard_read.initial.response.json", "protocol_version"),
            ("dashboard_read.initial.response.json", "event_schema_version"),
            ("dashboard_read.delta.response.json", "protocol_version"),
            ("dashboard_read.delta.response.json", "event_schema_version"),
        ] {
            var mismatchPresentation = DashboardPresentationState()
            try mismatchPresentation.apply(initialDashboardFixture)
            let beforeInstance = mismatchPresentation.coreInstanceId
            let beforeCursor = mismatchPresentation.cursor
            let beforeRuns = mismatchPresentation.runsById
            let mismatched = try decodeDashboardFixture(
                at: "\(fixtureRoot)/\(fixtureName)",
                replacing: field,
                with: "999.0"
            )
            do {
                try mismatchPresentation.apply(mismatched)
                throw NativeHealthTestFailure.failed(
                    "native Dashboard must reject mismatched \(field)"
                )
            } catch let error as DashboardPresentationError {
                try require(
                    error == .contractMismatch,
                    "native Dashboard must type mismatched \(field) as a contract failure"
                )
            }
            try require(
                mismatchPresentation.coreInstanceId == beforeInstance
                    && mismatchPresentation.cursor == beforeCursor
                    && mismatchPresentation.runsById == beforeRuns,
                "contract mismatch must not mutate native Dashboard state"
            )
        }
        try presentation.apply(resyncDashboardFixture)
        try require(
            presentation.runsById.isEmpty
                && presentation.coreInstanceId == resyncFixture.coreInstanceId,
            "native resync snapshot must replace stale presentation state"
        )
        var unknownPresentation = DashboardPresentationState()
        try unknownPresentation.apply(unavailableChangesDashboardFixture)
        let unknownRun = try unknownPresentation.runs(in: .working)[0]
        guard
            unknownRun.activity == "Unknown",
            case let .unavailable(reason) = unknownRun.changes
        else {
            throw NativeHealthTestFailure.failed(
                "native presentation must preserve Unknown and unavailable facts"
            )
        }
        try require(
            reason == "git_observation_not_configured",
            "native presentation must not invent zero changes"
        )
        _ = try decodeFixture(
            FlitRunDetailReadRequest.self,
            at: "\(fixtureRoot)/run_detail_read.request.json"
        )
        let runDetailFixture = try decodeFixture(
            FlitRunDetailReadResponse.self,
            at: "\(fixtureRoot)/run_detail_read.response.json"
        )
        _ = try decodeFixture(
            FlitManagedRunOpenInProviderRequest.self,
            at: "\(fixtureRoot)/managed_run_open_in_provider.request.json"
        )
        _ = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/run_detail_and_provider_open_errors.json"
        )
        try require(
            runDetailFixture.historyStatus == .unsupported
                && runDetailFixture.openInProviderStatus == .unsupported
                && runDetailFixture.events.count == 2
                && runDetailFixture.events[0].sourceKind == .core
                && runDetailFixture.events.allSatisfy({ $0.category == .lifecycle }),
            "generated Run detail must preserve structured evidence and capability facts"
        )
        let runDetailFixtureData = try Data(
            contentsOf: URL(fileURLWithPath: "\(fixtureRoot)/run_detail_read.response.json")
        )
        guard
            var runDetailObject = try JSONSerialization.jsonObject(with: runDetailFixtureData)
                as? [String: Any],
            var runDetailEvents = runDetailObject["events"] as? [[String: Any]]
        else {
            throw NativeHealthTestFailure.failed("Run detail fixture must contain events")
        }
        runDetailEvents[0].removeValue(forKey: "category")
        runDetailObject["events"] = runDetailEvents
        try requireDecodingFailure(
            FlitRunDetailReadResponse.self,
            from: JSONSerialization.data(withJSONObject: runDetailObject),
            "generated Run detail must reject a missing required evidence category"
        )
        runDetailEvents[0]["category"] = "future_category"
        runDetailObject["events"] = runDetailEvents
        try requireDecodingFailure(
            FlitRunDetailReadResponse.self,
            from: JSONSerialization.data(withJSONObject: runDetailObject),
            "generated Run detail must reject an unrecognized evidence category"
        )
        var runDetailPresentation = RunDetailPresentationState()
        try runDetailPresentation.apply(
            runDetailFixture,
            requestedRunId: "run-dashboard-1",
            expectedRunVersion: 3,
            requestedAfterCursor: 0,
            requestedEventLimit: 2
        )
        try require(
            runDetailPresentation.runId == "run-dashboard-1"
                && runDetailPresentation.runVersion == 3
                && runDetailPresentation.nextCursor == 2
                && runDetailPresentation.hasMore
                && runDetailPresentation.events.map(\.cursor) == [1, 2]
                && runDetailPresentation.events.map(\.eventType)
                    == ["run.created", "run.start_requested"]
                && runDetailPresentation.events.map(\.category)
                    == [.lifecycle, .lifecycle],
            "native Run detail must preserve exact identity and chronological locator order"
        )
        let categorizedRows = [
            FlitRunEvidenceCategory.activity,
            .command,
            .file,
            .test,
            .attention,
            .lifecycle,
            .unknown,
        ].enumerated().map { index, category in
            RunActivityRow(
                cursor: UInt64(index + 1),
                eventId: "event-category-\(index)",
                eventType: "fixture.category.\(index)",
                category: category,
                sourceKind: .core,
                confidence: 1.0,
                observedAt: "2026-08-05T00:00:0\(index).000Z"
            )
        }
        try require(
            RunActivityFilter.allCases == [
                .all,
                .activity,
                .command,
                .file,
                .test,
                .attention,
                .lifecycle,
            ],
            "native Run detail filter order must match the documented category choices"
        )
        for (filter, expectedCategories) in [
            (RunActivityFilter.all, categorizedRows.map(\.category)),
            (.activity, [.activity]),
            (.command, [.command]),
            (.file, [.file]),
            (.test, [.test]),
            (.attention, [.attention]),
            (.lifecycle, [.lifecycle]),
        ] {
            try require(
                filter.visibleRows(in: categorizedRows).map(\.category) == expectedCategories,
                "native Run detail filter must select only its exact generated category"
            )
        }
        try require(
            RunActivityFilter.all.visibleRows(in: categorizedRows).last?.category == .unknown,
            "unknown evidence must remain visible in All without being reclassified"
        )
        let groupingRows = [
            FlitRunEvidenceCategory.activity,
            .activity,
            .command,
            .activity,
            .unknown,
            .unknown,
        ].enumerated().map { index, category in
            RunActivityRow(
                cursor: UInt64(index + 1),
                eventId: "event-group-\(index)",
                eventType: "fixture.group.\(index)",
                category: category,
                sourceKind: .core,
                confidence: 1.0,
                observedAt: "2026-08-05T01:00:0\(index).000Z"
            )
        }
        let allGroups = RunActivityFilter.all.visibleGroups(in: groupingRows)
        try require(
            allGroups.map(\.category)
                == [.activity, .command, .activity, .unknown, .unknown]
                && allGroups.map(\.events.count) == [2, 1, 1, 1, 1]
                && allGroups[0].startedAt == "2026-08-05T01:00:00.000Z"
                && allGroups[0].endedAt == "2026-08-05T01:00:01.000Z",
            "native grouping must merge only adjacent exact known categories"
        )
        try require(
            RunActivityFilter.activity.visibleGroups(in: groupingRows).map(\.events.count)
                == [2, 1],
            "filtering must not merge same categories across an intervening hidden group"
        )
        let beforeRunId = runDetailPresentation.runId
        let beforeCursor = runDetailPresentation.nextCursor
        let beforeEventIds = runDetailPresentation.events.map(\.eventId)
        let mismatchedRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["run_id"] = "run-mismatch"
        }
        do {
            try runDetailPresentation.apply(
                mismatchedRunDetail,
                requestedRunId: "run-dashboard-1",
                expectedRunVersion: 3,
                requestedAfterCursor: 0,
                requestedEventLimit: 2
            )
            throw NativeHealthTestFailure.failed(
                "native Run detail must reject mismatched Run identity"
            )
        } catch let error as RunDetailPresentationError {
            try require(
                error == .runIdentityMismatch,
                "Run detail identity mismatch must remain typed"
            )
        }
        let reversedRunDetail = try changingRunDetail(runDetailFixture) { object in
            guard let events = object["events"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Run detail events must be an array")
            }
            object["events"] = Array(events.reversed())
        }
        do {
            try runDetailPresentation.apply(
                reversedRunDetail,
                requestedRunId: "run-dashboard-1",
                expectedRunVersion: 3,
                requestedAfterCursor: 0,
                requestedEventLimit: 2
            )
            throw NativeHealthTestFailure.failed(
                "native Run detail must reject non-monotonic event order"
            )
        } catch let error as RunDetailPresentationError {
            try require(
                error == .invalidEvent,
                "Run detail order mismatch must remain typed"
            )
        }
        let contractMismatchRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["protocol_version"] = "999.0"
        }
        let versionMismatchRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["run_version"] = 4
        }
        let cursorMismatchRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["next_cursor"] = 3
        }
        let duplicateRunDetail = try changingRunDetail(runDetailFixture) { object in
            guard var events = object["events"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Run detail events must be an array")
            }
            events[1]["event_id"] = events[0]["event_id"]
            object["events"] = events
        }
        let missingMoreRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["has_more"] = false
        }
        let exhaustedButMoreRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["run_version"] = 2
        }
        for (invalid, expectedVersion, requestedLimit, expectedError) in [
            (
                contractMismatchRunDetail,
                UInt64(3),
                UInt32(2),
                RunDetailPresentationError.contractMismatch
            ),
            (
                versionMismatchRunDetail,
                UInt64(3),
                UInt32(2),
                RunDetailPresentationError.runVersionMismatch
            ),
            (
                cursorMismatchRunDetail,
                UInt64(3),
                UInt32(2),
                RunDetailPresentationError.cursorMismatch
            ),
            (
                duplicateRunDetail,
                UInt64(3),
                UInt32(2),
                RunDetailPresentationError.duplicateEvent
            ),
            (
                missingMoreRunDetail,
                UInt64(3),
                UInt32(2),
                RunDetailPresentationError.cursorMismatch
            ),
            (
                exhaustedButMoreRunDetail,
                UInt64(2),
                UInt32(2),
                RunDetailPresentationError.cursorMismatch
            ),
            (
                runDetailFixture,
                UInt64(3),
                UInt32(50),
                RunDetailPresentationError.cursorMismatch
            ),
        ] {
            do {
                try runDetailPresentation.apply(
                    invalid,
                    requestedRunId: "run-dashboard-1",
                    expectedRunVersion: expectedVersion,
                    requestedAfterCursor: 0,
                    requestedEventLimit: requestedLimit
                )
                throw NativeHealthTestFailure.failed(
                    "native Run detail must reject every invalid delivery boundary"
                )
            } catch let error as RunDetailPresentationError {
                try require(
                    error == expectedError,
                    "Run detail invalid delivery must retain its typed error"
                )
            }
        }
        let fullPageRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["run_version"] = 51
            object["next_cursor"] = 50
            object["has_more"] = true
            object["events"] = try (1 ... 50).map { cursor in
                try runEvidenceObject(
                    cursor: UInt64(cursor),
                    eventId: "event-full-\(cursor)",
                    sessionId: nil,
                    eventType: "command.started",
                    category: .command,
                    sourceKind: .providerAdapter,
                    confidence: 1.0,
                    observedAt: "2026-07-28T00:00:00.000Z"
                )
            }
        }
        var fullPagePresentation = RunDetailPresentationState()
        try fullPagePresentation.apply(
            fullPageRunDetail,
            requestedRunId: "run-dashboard-1",
            expectedRunVersion: 51,
            requestedAfterCursor: 0,
            requestedEventLimit: 50
        )
        try require(
            fullPagePresentation.hasMore
                && fullPagePresentation.events.count == 50
                && fullPagePresentation.nextCursor == 50,
            "a full bounded page may truthfully report a later exact Run event"
        )
        let finalPageRunDetail = try changingRunDetail(runDetailFixture) { object in
            object["run_version"] = 51
            object["next_cursor"] = 51
            object["has_more"] = false
            object["events"] = [
                try runEvidenceObject(
                    cursor: 51,
                    eventId: "event-full-51",
                    sessionId: "session-dashboard-1",
                    eventType: "run.completed",
                    category: .lifecycle,
                    sourceKind: .providerAdapter,
                    confidence: 1.0,
                    observedAt: "2026-07-28T00:00:51.000Z"
                ),
            ]
        }
        let matchingFinalPageRunDetail = try changingRunDetail(finalPageRunDetail) { object in
            guard var events = object["events"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Run detail events must be an array")
            }
            events[0]["event_type"] = "command.finished"
            events[0]["category"] = "command"
            object["events"] = events
        }
        var crossPageGroupingPresentation = fullPagePresentation
        try crossPageGroupingPresentation.append(
            matchingFinalPageRunDetail,
            requestedRunId: "run-dashboard-1",
            expectedRunVersion: 51,
            requestedAfterCursor: 50,
            requestedEventLimit: 50
        )
        try require(
            RunActivityFilter.all.visibleGroups(
                in: crossPageGroupingPresentation.events
            ).map(\.events.count) == [51],
            "matching categories at an accepted page boundary must form one segment"
        )
        let duplicateFinalPage = try changingRunDetail(finalPageRunDetail) { object in
            guard var events = object["events"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Run detail events must be an array")
            }
            events[0]["event_id"] = "event-full-1"
            object["events"] = events
        }
        let changedCapabilityFinalPage = try changingRunDetail(finalPageRunDetail) { object in
            object["history_status"] = "supported"
        }
        for (invalidPage, expectedError) in [
            (duplicateFinalPage, RunDetailPresentationError.duplicateEvent),
            (changedCapabilityFinalPage, RunDetailPresentationError.contractMismatch),
        ] {
            do {
                try fullPagePresentation.append(
                    invalidPage,
                    requestedRunId: "run-dashboard-1",
                    expectedRunVersion: 51,
                    requestedAfterCursor: 50,
                    requestedEventLimit: 50
                )
                throw NativeHealthTestFailure.failed(
                    "native Run detail must reject inconsistent next-page facts"
                )
            } catch let error as RunDetailPresentationError {
                try require(
                    error == expectedError,
                    "invalid next page must retain its typed error"
                )
            }
        }
        try require(
            fullPagePresentation.events.count == 50
                && fullPagePresentation.nextCursor == 50
                && fullPagePresentation.hasMore,
            "invalid next page must preserve all accepted Run detail rows"
        )
        try fullPagePresentation.append(
            finalPageRunDetail,
            requestedRunId: "run-dashboard-1",
            expectedRunVersion: 51,
            requestedAfterCursor: 50,
            requestedEventLimit: 50
        )
        try require(
            fullPagePresentation.events.count == 51
                && fullPagePresentation.events.last?.cursor == 51
                && fullPagePresentation.nextCursor == 51
                && !fullPagePresentation.hasMore,
            "valid next page must append once and close at the exact Run tail"
        )
        let completedRunDetailFixture = try changingRunDetail(runDetailFixture) { object in
            guard var events = object["events"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Run detail events must be an array")
            }
            events.append(
                try runEvidenceObject(
                    cursor: 3,
                    eventId: "event-dashboard-completed",
                    sessionId: "session-dashboard-1",
                    eventType: "run.completed",
                    category: .lifecycle,
                    sourceKind: .providerAdapter,
                    confidence: 1.0,
                    observedAt: "2026-07-28T00:00:02.000Z"
                )
            )
            object["events"] = events
            object["next_cursor"] = 3
            object["has_more"] = false
        }
        let completedDashboardFixture = try changingDashboard(initialDashboardFixture) {
            object in
            guard var runs = object["runs"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Dashboard runs must be an array")
            }
            runs[0]["lifecycle"] = "Finished"
            runs[0]["dashboard_bucket"] = "Finished"
            runs[0]["ended_at"] = "2026-07-28T00:00:03.000Z"
            object["runs"] = runs
        }
        guard case let .snapshot(completedDashboardSnapshot) = completedDashboardFixture else {
            throw NativeHealthTestFailure.failed(
                "completed Dashboard fixture must remain a snapshot"
            )
        }
        let activeCompletionSummary = try runCompletionSummary(
            for: initialSnapshotFixture.runs[0]
        )
        try require(
            activeCompletionSummary == nil,
            "an active Dashboard Run must not produce a completion summary"
        )
        guard
            let completionSummary = try runCompletionSummary(
                for: completedDashboardSnapshot.runs[0]
            )
        else {
            throw NativeHealthTestFailure.failed(
                "a valid terminal Dashboard Run must produce a completion summary"
            )
        }
        try require(
            completionSummary.result == "Finished"
                && completionSummary.projectDisplayName == "Dashboard Project"
                && completionSummary.provider == .codex
                && completionSummary.startedAt == "2026-07-28T00:00:01.000Z"
                && completionSummary.endedAt == "2026-07-28T00:00:03.000Z"
                && completionSummary.changes == initialSnapshotFixture.runs[0].changes,
            "completion summary must preserve exact projected terminal facts"
        )
        let malformedCompletionFixtures = try [
            changingDashboard(completedDashboardFixture) { object in
                guard var runs = object["runs"] as? [[String: Any]] else {
                    throw NativeHealthTestFailure.failed("Dashboard runs must be an array")
                }
                runs[0]["ended_at"] = NSNull()
                object["runs"] = runs
            },
            changingDashboard(completedDashboardFixture) { object in
                guard var runs = object["runs"] as? [[String: Any]] else {
                    throw NativeHealthTestFailure.failed("Dashboard runs must be an array")
                }
                runs[0]["lifecycle"] = "UnknownTerminal"
                object["runs"] = runs
            },
        ]
        for malformed in malformedCompletionFixtures {
            guard case let .snapshot(snapshot) = malformed else {
                throw NativeHealthTestFailure.failed(
                    "malformed completion fixture must remain a snapshot"
                )
            }
            do {
                _ = try runCompletionSummary(for: snapshot.runs[0])
                throw NativeHealthTestFailure.failed(
                    "malformed terminal facts must not produce a completion summary"
                )
            } catch let error as RunCompletionSummaryError {
                try require(
                    error == .invalidProjection,
                    "malformed terminal facts must retain their typed failure"
                )
            }
        }
        let pagedDashboardFixture = try changingDashboard(initialDashboardFixture) { object in
            guard var runs = object["runs"] as? [[String: Any]] else {
                throw NativeHealthTestFailure.failed("Dashboard runs must be an array")
            }
            runs[0]["version"] = 51
            object["runs"] = runs
            object["next_cursor"] = 51
        }
        try require(
            runDetailPresentation.runId == beforeRunId
                && runDetailPresentation.nextCursor == beforeCursor
                && runDetailPresentation.events.map(\.eventId) == beforeEventIds,
            "invalid Run detail must not partially mutate native presentation state"
        )
        let invalidChangeVariants: [(String, String, Any)] = [
            (
                "dashboard_read.initial.response.json",
                "reason",
                "not_unavailable"
            ),
            (
                "dashboard_read.unavailable_changes.response.json",
                "files",
                0
            ),
        ]
        for (name, forbiddenField, forbiddenValue) in invalidChangeVariants {
            let data = try Data(
                contentsOf: URL(fileURLWithPath: "\(fixtureRoot)/\(name)")
            )
            guard
                var object = try JSONSerialization.jsonObject(with: data)
                    as? [String: Any],
                var runs = object["runs"] as? [[String: Any]],
                var changes = runs[0]["changes"] as? [String: Any]
            else {
                throw NativeHealthTestFailure.failed(
                    "Dashboard change fixture must contain one Run"
                )
            }
            changes[forbiddenField] = forbiddenValue
            runs[0]["changes"] = changes
            object["runs"] = runs
            try requireDecodingFailure(
                FlitDashboardReadResponse.self,
                from: try JSONSerialization.data(withJSONObject: object),
                "Dashboard changes availability must reject \(forbiddenField)"
            )
        }
        let invalidAttributionVariants: [(String, String?)] = [
            ("dashboard_read.initial.response.json", nil),
            ("dashboard_read.initial.response.json", "guessed"),
            ("dashboard_read.unavailable_changes.response.json", "exact"),
        ]
        for (name, attribution) in invalidAttributionVariants {
            let data = try Data(
                contentsOf: URL(fileURLWithPath: "\(fixtureRoot)/\(name)")
            )
            guard
                var object = try JSONSerialization.jsonObject(with: data)
                    as? [String: Any],
                var runs = object["runs"] as? [[String: Any]],
                var changes = runs[0]["changes"] as? [String: Any]
            else {
                throw NativeHealthTestFailure.failed(
                    "Dashboard attribution fixture must contain one Run"
                )
            }
            if let attribution {
                changes["attribution"] = attribution
            } else {
                changes.removeValue(forKey: "attribution")
            }
            runs[0]["changes"] = changes
            object["runs"] = runs
            try requireDecodingFailure(
                FlitDashboardReadResponse.self,
                from: try JSONSerialization.data(withJSONObject: object),
                "Dashboard changes must reject missing, unknown, or mixed attribution"
            )
        }
        for (name, requiredField) in [
            ("dashboard_read.initial.response.json", "runs"),
            ("dashboard_read.delta.response.json", "events"),
            ("dashboard_read.delta.response.json", "runs"),
            ("run_detail_read.response.json", "events"),
            ("run_detail_read.response.json", "history_status"),
            ("run_detail_read.response.json", "open_in_provider_status"),
        ] {
            let data = try Data(
                contentsOf: URL(fileURLWithPath: "\(fixtureRoot)/\(name)")
            )
            guard
                var object = try JSONSerialization.jsonObject(with: data)
                    as? [String: Any]
            else {
                throw NativeHealthTestFailure.failed(
                    "Dashboard delivery fixture must be an object"
                )
            }
            object.removeValue(forKey: requiredField)
            try requireDecodingFailure(
                FlitDashboardReadResponse.self,
                from: try JSONSerialization.data(withJSONObject: object),
                "Dashboard delivery must require \(requiredField)"
            )
        }
        _ = try decodeFixture(
            FlitProviderDiagnosticsRequest.self,
            at: "\(fixtureRoot)/provider_diagnostics.request.json"
        )
        _ = try decodeFixture(
            FlitQuitImpactRequest.self,
            at: "\(fixtureRoot)/quit_impact.request.json"
        )
        _ = try decodeFixture(
            FlitGitObservationRequest.self,
            at: "\(fixtureRoot)/git_observe.request.json"
        )
        for name in [
            "git_observe.not_repository.response.json",
            "git_observe.bare_repository.response.json",
            "git_observe.unborn.response.json",
            "git_observe.repository.response.json",
            "git_observe.runner_unavailable.response.json",
            "git_observe.git_unavailable.response.json",
            "git_observe.project_changed.response.json",
            "git_observe.process_unavailable.response.json",
            "git_observe.malformed_output.response.json",
        ] {
            _ = try decodeFixture(
                FlitGitObservationResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
        }
        let repositoryGitData = try Data(
            contentsOf: URL(
                fileURLWithPath: "\(fixtureRoot)/git_observe.repository.response.json"
            )
        )
        guard
            var mixedRepository = try JSONSerialization.jsonObject(with: repositoryGitData)
                as? [String: Any]
        else {
            throw NativeHealthTestFailure.failed("repository Git fixture must be an object")
        }
        mixedRepository["reason"] = "runner_unavailable"
        try requireDecodingFailure(
            FlitGitObservationResponse.self,
            from: try JSONSerialization.data(withJSONObject: mixedRepository),
            "repository Git observation must reject unavailable fields"
        )
        let unavailableGitData = try Data(
            contentsOf: URL(
                fileURLWithPath:
                    "\(fixtureRoot)/git_observe.runner_unavailable.response.json"
            )
        )
        guard
            var mixedUnavailable = try JSONSerialization.jsonObject(with: unavailableGitData)
                as? [String: Any]
        else {
            throw NativeHealthTestFailure.failed("unavailable Git fixture must be an object")
        }
        mixedUnavailable["canonical_root"] = "/private/tmp/invented"
        mixedUnavailable["head"] = ["availability": "unborn"]
        mixedUnavailable["dirty"] = [
            "staged": 0,
            "unstaged": 0,
            "untracked": 0,
            "entries": 0,
        ]
        try requireDecodingFailure(
            FlitGitObservationResponse.self,
            from: try JSONSerialization.data(withJSONObject: mixedUnavailable),
            "unavailable Git observation must reject repository fields"
        )
        let quitImpactFixture = try decodeFixture(
            FlitQuitImpactResponse.self,
            at: "\(fixtureRoot)/quit_impact.response.json"
        )
        try require(
            quitImpactFixture.runs.map(\.executionAfterQuit)
                == [.continues, .stops, .unknown]
                && quitImpactFixture.runs.map(\.reason)
                    == [
                        .capabilitySupported,
                        .capabilityUnsupported,
                        .capabilityMissing,
                    ],
            "generated Quit impact contract must preserve exact per-Run outcomes"
        )
        let exactQuitContent = ExplicitQuitAlertContent.make(for: .exact(quitImpactFixture))
        try require(
            quitImpactFixture.runs.allSatisfy { exactQuitContent.message.contains($0.title) }
                && exactQuitContent.message.contains("continues in Codex")
                && exactQuitContent.message.contains("stops when Flit quits (Codex)")
                && exactQuitContent.message.contains(
                    "outcome after Quit is unknown (Codex)"
                )
                && exactQuitContent.message.contains(
                    FoundationCopy.text(.quitMonitoringBoundary)
                ),
            "active Quit copy must render every exact Run outcome and the Flit boundary"
        )
        let unavailableQuitContent = ExplicitQuitAlertContent.make(for: .unavailable)
        try require(
            unavailableQuitContent.message.contains(
                FoundationCopy.text(.quitImpactUnavailable)
            )
                && unavailableQuitContent.message.contains(
                    FoundationCopy.text(.quitMonitoringBoundary)
                ),
            "unavailable Quit copy must disclose unknown provider impact and the Flit boundary"
        )

        let explicitQuitWindow = NSWindow()
        let emptyQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let emptyQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { .exact(emptyQuitImpact) },
            presenter: emptyQuitPresenter
        )
        var emptyQuitDecisions: [Bool] = []
        let emptyQuitDisposition = emptyQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { emptyQuitDecisions.append($0) }
        )
        try require(
            emptyQuitDisposition == .terminateNow
                && emptyQuitDecisions.isEmpty
                && emptyQuitPresenter.contents.isEmpty,
            "an exact empty Quit preview must terminate immediately without an alert"
        )

        let activeQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let activeQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { .exact(quitImpactFixture) },
            presenter: activeQuitPresenter
        )
        var activeQuitDecisions: [Bool] = []
        let activeQuitDisposition = activeQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { activeQuitDecisions.append($0) }
        )
        let duplicateQuitDisposition = activeQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { activeQuitDecisions.append($0) }
        )
        try require(
            activeQuitDisposition == .pending
                && duplicateQuitDisposition == .pending
                && activeQuitPresenter.contents == [exactQuitContent]
                && activeQuitDecisions.isEmpty,
            "an active Quit preview must present one alert and suppress duplicate requests"
        )
        activeQuitPresenter.choose(.confirm)
        try require(
            activeQuitDecisions == [true],
            "an unchanged active Quit preview must terminate after confirmation"
        )

        let changedQuitImpact = try changingQuitImpact(
            quitImpactFixture,
            cursor: quitImpactFixture.cursor + 1
        )
        var staleQuitPreviews: [ExplicitQuitPreview] = [
            .exact(quitImpactFixture),
            .exact(changedQuitImpact),
            .exact(changedQuitImpact),
        ]
        let staleQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let staleQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { staleQuitPreviews.removeFirst() },
            presenter: staleQuitPresenter
        )
        var staleQuitDecisions: [Bool] = []
        _ = staleQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { staleQuitDecisions.append($0) }
        )
        staleQuitPresenter.choose(.confirm)
        try require(
            staleQuitPresenter.contents.count == 2 && staleQuitDecisions.isEmpty,
            "a changed Quit preview must replace the alert without terminating"
        )
        staleQuitPresenter.choose(.confirm)
        try require(
            staleQuitDecisions == [true],
            "the changed Quit preview must require a fresh matching confirmation"
        )

        var clearedQuitPreviews: [ExplicitQuitPreview] = [
            .exact(quitImpactFixture),
            .exact(emptyQuitImpact),
            .exact(emptyQuitImpact),
        ]
        let clearedQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let clearedQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { clearedQuitPreviews.removeFirst() },
            presenter: clearedQuitPresenter
        )
        var clearedQuitDecisions: [Bool] = []
        _ = clearedQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { clearedQuitDecisions.append($0) }
        )
        clearedQuitPresenter.choose(.confirm)
        try require(
            clearedQuitPresenter.contents.count == 2
                && clearedQuitPresenter.contents[1].message.contains(
                    FoundationCopy.text(.quitNoActiveRuns)
                )
                && clearedQuitDecisions.isEmpty,
            "a stale preview that becomes empty must still require fresh confirmation"
        )
        clearedQuitPresenter.choose(.confirm)
        try require(
            clearedQuitDecisions == [true],
            "the newly empty preview must terminate only after its own confirmation"
        )

        let cancelledQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let cancelledQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { .exact(quitImpactFixture) },
            presenter: cancelledQuitPresenter
        )
        var cancelledQuitDecisions: [Bool] = []
        _ = cancelledQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { cancelledQuitDecisions.append($0) }
        )
        cancelledQuitPresenter.choose(.cancel)
        try require(
            cancelledQuitDecisions == [false],
            "cancelling an explicit Quit must never terminate"
        )

        let unavailableQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let unavailableQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { .unavailable },
            presenter: unavailableQuitPresenter
        )
        var unavailableQuitDecisions: [Bool] = []
        _ = unavailableQuitCoordinator.requestQuit(
            for: explicitQuitWindow,
            completion: { unavailableQuitDecisions.append($0) }
        )
        unavailableQuitPresenter.choose(.confirm)
        try require(
            unavailableQuitPresenter.contents == [unavailableQuitContent]
                && unavailableQuitDecisions == [true],
            "repeated unavailable Quit reads must require explicit unknown-impact confirmation"
        )
        for (name, compatibility) in [
            (
                "provider_diagnostics.supported.response.json",
                FlitProviderCompatibility.supported
            ),
            (
                "provider_diagnostics.unknown.response.json",
                FlitProviderCompatibility.unknown
            ),
            (
                "provider_diagnostics.unavailable.response.json",
                FlitProviderCompatibility.unavailable
            ),
        ] {
            let response = try decodeFixture(
                FlitProviderDiagnosticsResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                response.compatibility == compatibility
                    && response.capabilities.count == 16,
                "generated provider diagnostics must decode \(compatibility)"
            )
        }
        let managedRunRequest = try decodeFixture(
            FlitManagedRunStartRequest.self,
            at: "\(fixtureRoot)/managed_run_start.request.json"
        )
        try require(
            managedRunRequest.permissionMode == .manual
                && managedRunRequest.permissionModeVersion == 1
                && managedRunRequest.gitBaselineEventId
                    == "event-run-managed-1-git-baseline"
                && managedRunRequest.gitBaselineObservedAt
                    == "2026-07-27T12:00:00Z",
            "generated managed Run request must preserve exact Manual mode and Git baseline identity"
        )
        let managedRunResponse = try decodeFixture(
            FlitManagedRunStartResponse.self,
            at: "\(fixtureRoot)/managed_run_start.response.json"
        )
        try require(
            managedRunResponse.providerThreadId == "codex-thread-1"
                && managedRunResponse.providerTurnId == "codex-turn-1",
            "generated managed Run response must preserve provider identities"
        )
        let providerAutoRequest = try decodeFixture(
            FlitManagedRunStartRequest.self,
            at: "\(fixtureRoot)/managed_run_start.provider_auto.request.json"
        )
        let providerAutoResponse = try decodeFixture(
            FlitManagedRunStartResponse.self,
            at: "\(fixtureRoot)/managed_run_start.provider_auto.response.json"
        )
        try require(
            providerAutoRequest.permissionMode == .providerAuto
                && providerAutoResponse.permissionMode == .providerAuto
                && providerAutoResponse.providerConfiguration
                    == "readOnly+on-request+auto_review",
            "generated managed Run contract must preserve exact ProviderAuto configuration"
        )
        let managedRunErrors = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/managed_run_errors.json"
        )
        try require(
            managedRunErrors.count == 6,
            "generated managed Run errors must decode every public failure"
        )
        _ = try decodeFixture(
            FlitManagedRunObserveRequest.self,
            at: "\(fixtureRoot)/managed_run_observe.request.json"
        )
        for (name, expectedStatus) in [
            (
                "managed_run_observe.permission_requested.response.json",
                FlitManagedRunObservationStatus.permissionRequested
            ),
            (
                "managed_run_observe.provider_outcome_resolved.response.json",
                FlitManagedRunObservationStatus.providerOutcomeResolved
            ),
            (
                "managed_run_observe.turn_completed.response.json",
                FlitManagedRunObservationStatus.turnCompleted
            ),
            (
                "managed_run_observe.turn_interrupted.response.json",
                FlitManagedRunObservationStatus.turnInterrupted
            ),
        ] {
            let observation = try decodeFixture(
                FlitManagedRunObserveResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                observation.status == expectedStatus,
                "generated managed Run observation must decode \(expectedStatus)"
            )
        }
        let permissionObservationData = try Data(
            contentsOf: URL(
                fileURLWithPath:
                    "\(fixtureRoot)/managed_run_observe.permission_requested.response.json"
            )
        )
        guard
            let permissionObservation = try JSONSerialization.jsonObject(
                with: permissionObservationData
            ) as? [String: Any]
        else {
            throw NativeHealthTestFailure.failed("permission observation must be an object")
        }
        for requiredField in [
            "provider_item_id",
            "provider_request_id",
            "request_id",
            "request_version",
        ] {
            var missing = permissionObservation
            missing.removeValue(forKey: requiredField)
            try requireDecodingFailure(
                FlitManagedRunObserveResponse.self,
                from: try JSONSerialization.data(withJSONObject: missing),
                "permission observation must require \(requiredField)"
            )
        }
        var permissionWithTerminalField = permissionObservation
        permissionWithTerminalField["event_version"] = 4
        try requireDecodingFailure(
            FlitManagedRunObserveResponse.self,
            from: try JSONSerialization.data(withJSONObject: permissionWithTerminalField),
            "permission observation must reject terminal fields"
        )
        let providerOutcomeData = try Data(
            contentsOf: URL(
                fileURLWithPath:
                    "\(fixtureRoot)/managed_run_observe.provider_outcome_resolved.response.json"
            )
        )
        guard
            let providerOutcome = try JSONSerialization.jsonObject(
                with: providerOutcomeData
            ) as? [String: Any]
        else {
            throw NativeHealthTestFailure.failed("provider outcome must be an object")
        }
        for requiredField in [
            "provider_item_id",
            "provider_decision_id",
            "request_id",
            "request_version",
            "request_event_id",
            "provider_decision",
            "terminal_outcome",
            "event_version",
        ] {
            var missing = providerOutcome
            missing.removeValue(forKey: requiredField)
            try requireDecodingFailure(
                FlitManagedRunObserveResponse.self,
                from: try JSONSerialization.data(withJSONObject: missing),
                "provider outcome must require \(requiredField)"
            )
        }
        var providerOutcomeWithClientRequest = providerOutcome
        providerOutcomeWithClientRequest["provider_request_id"] = 7
        try requireDecodingFailure(
            FlitManagedRunObserveResponse.self,
            from: try JSONSerialization.data(withJSONObject: providerOutcomeWithClientRequest),
            "provider outcome must reject a client request identity"
        )
        let terminalObservationData = try Data(
            contentsOf: URL(
                fileURLWithPath:
                    "\(fixtureRoot)/managed_run_observe.turn_completed.response.json"
            )
        )
        guard
            let terminalObservation = try JSONSerialization.jsonObject(
                with: terminalObservationData
            ) as? [String: Any]
        else {
            throw NativeHealthTestFailure.failed("terminal observation must be an object")
        }
        var terminalWithoutVersion = terminalObservation
        terminalWithoutVersion.removeValue(forKey: "event_version")
        try requireDecodingFailure(
            FlitManagedRunObserveResponse.self,
            from: try JSONSerialization.data(withJSONObject: terminalWithoutVersion),
            "terminal observation must require event_version"
        )
        var terminalWithPermissionField = terminalObservation
        terminalWithPermissionField["provider_item_id"] = "cross-variant-item"
        try requireDecodingFailure(
            FlitManagedRunObserveResponse.self,
            from: try JSONSerialization.data(withJSONObject: terminalWithPermissionField),
            "terminal observation must reject permission fields"
        )
        let managedRunObserveErrors = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/managed_run_observe_errors.json"
        )
        try require(
            managedRunObserveErrors.count == 2,
            "generated managed Run observe errors must decode every public failure"
        )
        let managedRunObserveRequest = try String(
            contentsOfFile: "\(fixtureRoot)/managed_run_observe.request.json",
            encoding: .utf8
        )
        try requireCommandError(
            try managedRunObserveJson(requestJson: managedRunObserveRequest),
            code: .managedRunNotActive,
            fixtures: managedRunObserveErrors
        )
        _ = try decodeFixture(
            FlitManagedRunPermissionRespondRequest.self,
            at: "\(fixtureRoot)/managed_run_permission_respond.request.json"
        )
        for (name, expectedStatus) in [
            (
                "managed_run_permission_respond.delivered.response.json",
                FlitManagedRunPermissionResponseStatus.delivered
            ),
            (
                "managed_run_permission_respond.delivery_unknown.response.json",
                FlitManagedRunPermissionResponseStatus.deliveryUnknown
            ),
        ] {
            let response = try decodeFixture(
                FlitManagedRunPermissionRespondResponse.self,
                at: "\(fixtureRoot)/\(name)"
            )
            try require(
                response.status == expectedStatus,
                "generated permission response must decode \(expectedStatus)"
            )
        }
        let permissionResponseErrors = try decodeFixture(
            [FlitCommandError].self,
            at: "\(fixtureRoot)/managed_run_permission_respond_errors.json"
        )
        try require(
            permissionResponseErrors.count == 5,
            "generated permission response errors must decode every public failure"
        )
        let permissionResponseRequest = try String(
            contentsOfFile: "\(fixtureRoot)/managed_run_permission_respond.request.json",
            encoding: .utf8
        )
        try requireCommandError(
            try managedRunPermissionRespondJson(requestJson: permissionResponseRequest),
            code: .managedRunNotActive,
            fixtures: permissionResponseErrors
        )
        let driftedInspection = Data(
            """
            {
              "protocol_version": "\(flitClientProtocolVersion)",
              "canonical_path": "/tmp/flit-project",
              "selected_via_symlink": false
            }
            """.utf8
        )
        try requireDecodingFailure(
            FlitProjectInspectionResponse.self,
            from: driftedInspection,
            "generated Project decoding must reject a missing required field"
        )
        let driftedRegistration = Data(
            """
            {
              "protocol_version": "\(flitClientProtocolVersion)",
              "status": "invented_status",
              "project": null,
              "existing_project_id": null
            }
            """.utf8
        )
        try requireDecodingFailure(
            FlitProjectRegistrationResponse.self,
            from: driftedRegistration,
            "generated Project decoding must reject an unknown status"
        )
        let driftedDiagnostics = Data(
            """
            {
              "protocol_version": "\(flitClientProtocolVersion)",
              "provider": "codex",
              "compatibility": "unknown",
              "executable_version": "9.9.9",
              "fingerprint_mismatches": [],
              "unavailable_reason": null
            }
            """.utf8
        )
        try requireDecodingFailure(
            FlitProviderDiagnosticsResponse.self,
            from: driftedDiagnostics,
            "generated provider diagnostics must reject a missing capability list"
        )

        let projectDirectory = "\(root)/target/flit-macos/native-project"
        try FileManager.default.createDirectory(
            atPath: projectDirectory,
            withIntermediateDirectories: true
        )
        try requireCommandError(
            try projectInspectJson(
                selectedPath: "\(root)/target/flit-macos/missing-project",
                clientProtocolVersion: requestVersion
            ),
            code: .projectInspectionFailure,
            fixtures: commandErrors
        )
        try requireCommandError(
            try projectsListPageJson(
                afterDisplayName: "Project",
                afterProjectId: nil,
                limit: 50,
                clientProtocolVersion: requestVersion
            ),
            code: .invalidProjectRequest,
            fixtures: commandErrors
        )
        let inspected = try projectInspectJson(
            selectedPath: projectDirectory,
            clientProtocolVersion: requestVersion
        )
        let inspectedObject = try JSONDecoder().decode(
            FlitProjectInspectionResponse.self,
            from: Data(inspected.utf8)
        )
        try require(
            !inspectedObject.selectedViaSymlink,
            "native Project inspection must preserve direct selection"
        )
        let registered = try projectRegisterJson(
            projectId: "native-project",
            displayName: "Native Project",
            selectedPath: projectDirectory,
            createdAt: "2026-07-27T00:00:00.000Z",
            clientProtocolVersion: requestVersion
        )
        let registeredObject = try JSONDecoder().decode(
            FlitProjectRegistrationResponse.self,
            from: Data(registered.utf8)
        )
        try require(
            registeredObject.status == .registered,
            "native Project registration must return its typed status"
        )
        try requireCommandError(
            try gitObserveProjectJson(
                projectId: "native-project",
                clientProtocolVersion: requestVersion
            ),
            code: .projectNotTrusted,
            fixtures: commandErrors
        )
        let conflictDirectory = "\(root)/target/flit-macos/native-project-conflict"
        try FileManager.default.createDirectory(
            atPath: conflictDirectory,
            withIntermediateDirectories: true
        )
        try requireCommandError(
            try projectRegisterJson(
                projectId: "native-project",
                displayName: "Conflicting Native Project",
                selectedPath: conflictDirectory,
                createdAt: "2026-07-27T00:00:00.000Z",
                clientProtocolVersion: requestVersion
            ),
            code: .projectConflict,
            fixtures: commandErrors
        )
        try requireCommandError(
            try projectTrustJson(
                projectId: "missing-native-project",
                selectedPath: projectDirectory,
                confirmedAt: "2026-07-27T00:00:01.000Z",
                clientProtocolVersion: requestVersion
            ),
            code: .projectNotFound,
            fixtures: commandErrors
        )
        let trusted = try projectTrustJson(
            projectId: "native-project",
            selectedPath: projectDirectory,
            confirmedAt: "2026-07-27T00:00:01.000Z",
            clientProtocolVersion: requestVersion
        )
        let trustedObject = try JSONDecoder().decode(
            FlitProjectTrustResponse.self,
            from: Data(trusted.utf8)
        )
        try require(
            trustedObject.status == .trusted && trustedObject.project.trusted,
            "native Project trust must decode its exact typed result"
        )
        let gitObservation = try JSONDecoder().decode(
            FlitGitObservationResponse.self,
            from: Data(
                try gitObserveProjectJson(
                    projectId: "native-project",
                    clientProtocolVersion: requestVersion
                ).utf8
            )
        )
        guard case let .unavailable(unavailableGit) = gitObservation else {
            throw NativeHealthTestFailure.failed(
                "standalone native tests must not invent a bundled Git runner"
            )
        }
        try require(
            unavailableGit.reason == .runnerUnavailable,
            "standalone native Git observation must expose exact runner unavailability"
        )
        let listed = try projectsListPageJson(
            afterDisplayName: nil,
            afterProjectId: nil,
            limit: 50,
            clientProtocolVersion: requestVersion
        )
        let listedObject = try JSONDecoder().decode(
            FlitProjectsListResponse.self,
            from: Data(listed.utf8)
        )
        try require(
            listedObject.projects.count == 1 && listedObject.nextCursor == nil,
            "native Project list must return the registered active Project"
        )
        let driftDirectory = "\(root)/target/flit-macos/native-project-drift"
        let movedDriftDirectory = "\(driftDirectory)-moved"
        try FileManager.default.createDirectory(
            atPath: driftDirectory,
            withIntermediateDirectories: true
        )
        _ = try projectRegisterJson(
            projectId: "native-project-drift",
            displayName: "Native Project Drift",
            selectedPath: driftDirectory,
            createdAt: "2026-07-27T00:00:02.000Z",
            clientProtocolVersion: requestVersion
        )
        try FileManager.default.moveItem(
            atPath: driftDirectory,
            toPath: movedDriftDirectory
        )
        try FileManager.default.createDirectory(
            atPath: driftDirectory,
            withIntermediateDirectories: true
        )
        try requireCommandError(
            try projectTrustJson(
                projectId: "native-project-drift",
                selectedPath: driftDirectory,
                confirmedAt: "2026-07-27T00:00:03.000Z",
                clientProtocolVersion: requestVersion
            ),
            code: .projectIdentityMismatch,
            fixtures: commandErrors
        )

        guard case .ready = client.load() else {
            throw NativeHealthTestFailure.failed("repeated native health must remain ready")
        }
        try require(
            SystemHealthClient(clientProtocolVersion: "2.0").load()
                == .unavailable(messageKey: "errors.protocolMismatch"),
            "native client must fail closed when its generated version is stale"
        )

        let controller = FoundationViewController(client: client)
        _ = controller.view
        try require(controller.currentState == .ready, "foundation controller must render ready")
        try require(controller.hostedLeafCount == 1, "foundation must use one hosted SwiftUI leaf")
        try require(
            controller.view.identifier?.rawValue == "flit.foundation.root",
            "foundation root must expose a stable interface identifier"
        )
        try require(
            controller.view.accessibilityIdentifier() == "flit.foundation.root",
            "foundation root must expose a stable accessibility identifier"
        )
        let fixtureController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { unavailableChangesDashboardFixture }
            )
        )
        _ = fixtureController.view
        let fixtureViews = descendants(of: fixtureController.view)
        let fixtureIdentifiers = Set(
            fixtureViews.compactMap { $0.accessibilityIdentifier() }
        )
        try require(
            fixtureIdentifiers.contains("flit.dashboard.scroll")
                && fixtureIdentifiers.contains("flit.dashboard.section.Working")
                && fixtureIdentifiers.contains("flit.dashboard.run.\(unknownRun.runId)"),
            "fixture-backed Dashboard must expose stable scroll, section, and Run identifiers"
        )
        let fixtureCopy = fixtureViews.compactMap { ($0 as? NSTextField)?.stringValue }
        try require(
            fixtureCopy.contains(FoundationCopy.text(.dashboardActivityUnknown))
                && fixtureCopy.contains(
                    FoundationCopy.format(
                        .dashboardAttention,
                        unknownRun.attentionLevel,
                        unknownRun.attentionOpenCount
                    )
                )
                && fixtureCopy.contains(
                    FoundationCopy.format(.dashboardChangesUnavailable, reason)
                ),
            "fixture-backed Run card must render exact Unknown, attention, and unavailable facts"
        )
        let detailController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { completedDashboardFixture }
            ),
            runDetailClient: RunDetailClient(
                fixtureLoader: { request in
                    guard
                        request.runId == "run-dashboard-1",
                        request.expectedRunVersion == 3,
                        request.afterCursor == 0,
                        request.requestedEventLimit == 50,
                        request.clientProtocolVersion == flitClientProtocolVersion
                    else {
                        throw NativeHealthTestFailure.failed(
                            "Run detail action must construct the exact bounded current request"
                        )
                    }
                    return completedRunDetailFixture
                }
            )
        )
        _ = detailController.view
        let detailWindow = NSWindow(contentViewController: detailController)
        detailWindow.setContentSize(NSSize(width: 1_280, height: 720))
        detailWindow.contentView?.layoutSubtreeIfNeeded()
        let detailDashboardViews = descendants(of: detailController.view)
        guard
            let detailButton = detailDashboardViews.first(where: {
                $0.accessibilityIdentifier()
                    == "flit.dashboard.runDetail.run-dashboard-1"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed(
                "fixture-backed Run card must expose its Activity action"
            )
        }
        detailButton.performClick(nil)
        let detailViews = descendants(of: detailController.view)
        let detailIdentifiers = Set(
            detailViews.compactMap { $0.accessibilityIdentifier() }
        )
        try require(
            detailIdentifiers.contains("flit.runDetail.back")
                && detailIdentifiers.contains("flit.runDetail.title.run-dashboard-1")
                && detailIdentifiers.contains("flit.runDetail.completionSummary")
                && detailIdentifiers.contains("flit.runDetail.group.1.3")
                && detailIdentifiers.contains("flit.runDetail.event.1")
                && detailIdentifiers.contains("flit.runDetail.event.2")
                && detailIdentifiers.contains("flit.runDetail.event.3"),
            "native Run detail must expose stable back, title, and event identities"
        )
        guard
            let initialCategoryFilter = detailViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.filter"
            }) as? NSPopUpButton
        else {
            throw NativeHealthTestFailure.failed(
                "native Run detail must expose its category filter"
            )
        }
        try require(
            initialCategoryFilter.itemTitles == [
                FoundationCopy.text(.runDetailFilterAll),
                FoundationCopy.text(.runDetailFilterActivity),
                FoundationCopy.text(.runDetailFilterCommand),
                FoundationCopy.text(.runDetailFilterFile),
                FoundationCopy.text(.runDetailFilterTest),
                FoundationCopy.text(.runDetailFilterAttention),
                FoundationCopy.text(.runDetailFilterLifecycle),
            ]
                && initialCategoryFilter.titleOfSelectedItem
                    == FoundationCopy.text(.runDetailFilterAll),
            "native Run detail must default to All with stable documented filter choices"
        )
        guard
            let evidenceToggle = detailViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.evidenceToggle.1"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed(
                "every Run Activity event must expose its evidence control"
            )
        }
        try require(
            evidenceToggle.title == FoundationCopy.text(.runDetailShowEvidence),
            "collapsed Run evidence must use stable Show Evidence copy"
        )
        evidenceToggle.performClick(nil)
        let detailWithEvidenceViews = descendants(of: detailController.view)
        let detailWithEvidenceCopy = detailWithEvidenceViews.compactMap {
            ($0 as? NSTextField)?.stringValue
        }
        let hideEvidenceToggle = detailWithEvidenceViews.first(where: {
            $0.accessibilityIdentifier() == "flit.runDetail.evidenceToggle.1"
        }) as? NSButton
        try require(
            detailWithEvidenceViews.contains(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.evidence.1"
            })
                && detailWithEvidenceCopy.contains(
                    FoundationCopy.format(
                        .runDetailEvidence,
                        "event-dashboard-created",
                        "run.created",
                        FoundationCopy.text(.runDetailFilterLifecycle),
                        "core",
                        100,
                        "2026-07-28T00:00:00.000Z"
                    )
                )
                && detailWithEvidenceCopy.contains(
                    FoundationCopy.text(.runDetailRawPayloadUnavailable)
                )
                && hideEvidenceToggle?.title
                    == FoundationCopy.text(.runDetailHideEvidence)
                && detailWindow.firstResponder === hideEvidenceToggle
                && !detailWithEvidenceCopy.contains("session-dashboard-1"),
            "evidence disclosure must show bounded locator facts and explicit raw unavailability"
        )
        guard
            let categoryFilter = detailWithEvidenceViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.filter"
            }) as? NSPopUpButton
        else {
            throw NativeHealthTestFailure.failed(
                "evidence rerender must preserve the category filter"
            )
        }
        categoryFilter.selectItem(
            withTitle: FoundationCopy.text(.runDetailFilterCommand)
        )
        categoryFilter.sendAction(categoryFilter.action, to: categoryFilter.target)
        let commandFilteredViews = descendants(of: detailController.view)
        try require(
            !commandFilteredViews.contains(where: {
                $0.accessibilityIdentifier().hasPrefix("flit.runDetail.event.")
            })
                && commandFilteredViews.contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.completionSummary"
                })
                && commandFilteredViews.contains(where: {
                    $0.accessibilityIdentifier()
                        == "flit.runDetail.noMatchingEvents.command"
                }),
            "a category with no accepted rows must show a truthful filtered empty state"
        )
        guard
            let updatedCategoryFilter = commandFilteredViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.filter"
            }) as? NSPopUpButton
        else {
            throw NativeHealthTestFailure.failed(
                "filtered Run detail must preserve its category control"
            )
        }
        updatedCategoryFilter.selectItem(
            withTitle: FoundationCopy.text(.runDetailFilterLifecycle)
        )
        updatedCategoryFilter.sendAction(
            updatedCategoryFilter.action,
            to: updatedCategoryFilter.target
        )
        try require(
            descendants(of: detailController.view).filter({
                $0.accessibilityIdentifier().hasPrefix("flit.runDetail.event.")
            }).count == 3
                && descendants(of: detailController.view).contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.evidence.1"
                }),
            "selecting Lifecycle must restore its rows and expanded evidence"
        )
        let detailCopy = detailViews.compactMap { ($0 as? NSTextField)?.stringValue }
        try require(
            detailCopy.contains(FoundationCopy.text(.runDetailCompletionSummary))
                && detailCopy.contains(
                    FoundationCopy.format(.runDetailSummaryResult, "Finished")
                )
                && detailCopy.contains(
                    FoundationCopy.format(
                        .runDetailSummaryProjectProvider,
                        "Dashboard Project",
                        "codex"
                    )
                )
                && detailCopy.contains(
                    FoundationCopy.format(
                        .runDetailSummaryTime,
                        "2026-07-28T00:00:01.000Z",
                        "2026-07-28T00:00:03.000Z"
                    )
                )
                && detailCopy.contains(
                    FoundationCopy.format(.dashboardChanges, 3, 42, 7)
                )
                && detailCopy.contains(
                    FoundationCopy.text(.runDetailSummaryBranchUnavailable)
                )
                && detailCopy.contains(
                    FoundationCopy.text(.runDetailSummaryValidationUnavailable)
                )
                && detailCopy.contains(
                    FoundationCopy.text(.runDetailSummaryOpenIssuesUnavailable)
                )
                && detailCopy.contains(
                    FoundationCopy.text(.runDetailSummaryEvidenceUnavailable)
                )
                && detailCopy.contains(
                FoundationCopy.format(
                    .runDetailGroup,
                    "2026-07-28T00:00:00.000Z",
                    "2026-07-28T00:00:02.000Z",
                    FoundationCopy.text(.runDetailFilterLifecycle),
                    3
                )
            )
                && detailCopy.contains(
                FoundationCopy.format(
                    .runDetailEvent,
                    "2026-07-28T00:00:00.000Z",
                    "run.created",
                    "core",
                    100
                )
            )
                && detailCopy.contains(
                    FoundationCopy.format(
                        .runDetailCapability,
                        FoundationCopy.text(.runDetailProviderHistory),
                        "unsupported"
                    )
                )
                && !detailCopy.contains("session-dashboard-1"),
            "native Run detail must render bounded locator facts without session identity"
        )
        guard
            let backButton = detailViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.back"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed("native Run detail must expose Back")
        }
        backButton.performClick(nil)
        evidenceToggle.sendAction(evidenceToggle.action, to: evidenceToggle.target)
        try require(
            descendants(of: detailController.view).contains(where: {
                $0.accessibilityIdentifier() == "flit.dashboard.run.run-dashboard-1"
            }),
            "Back must restore the Dashboard and stale evidence controls must be inert"
        )
        let malformedDetailController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { malformedCompletionFixtures[0] }
            ),
            runDetailClient: RunDetailClient(
                fixtureLoader: { _ in completedRunDetailFixture }
            )
        )
        _ = malformedDetailController.view
        guard
            let malformedDetailButton = descendants(
                of: malformedDetailController.view
            ).first(where: {
                $0.accessibilityIdentifier()
                    == "flit.dashboard.runDetail.run-dashboard-1"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed(
                "malformed terminal Dashboard must still expose its bounded detail action"
            )
        }
        malformedDetailButton.performClick(nil)
        let malformedDetailViews = descendants(of: malformedDetailController.view)
        try require(
            malformedDetailViews.contains(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.unavailable"
            })
                && !malformedDetailViews.contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.completionSummary"
                }),
            "malformed terminal projection must fail Run detail closed"
        )
        let pagedController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { pagedDashboardFixture }
            ),
            runDetailClient: RunDetailClient(
                fixtureLoader: { request in
                    guard
                        request.runId == "run-dashboard-1",
                        request.expectedRunVersion == 51,
                        request.requestedEventLimit == 50,
                        request.clientProtocolVersion == flitClientProtocolVersion
                    else {
                        throw NativeHealthTestFailure.failed(
                            "paged Run detail request must preserve exact scope"
                        )
                    }
                    switch request.afterCursor {
                    case 0: return fullPageRunDetail
                    case 50: return finalPageRunDetail
                    default:
                        throw NativeHealthTestFailure.failed(
                            "paged Run detail must request only the accepted cursor"
                        )
                    }
                }
            )
        )
        _ = pagedController.view
        let pagedWindow = NSWindow(contentViewController: pagedController)
        pagedWindow.setContentSize(NSSize(width: 1_280, height: 720))
        pagedWindow.contentView?.layoutSubtreeIfNeeded()
        guard
            let pagedDetailButton = descendants(of: pagedController.view).first(where: {
                $0.accessibilityIdentifier()
                    == "flit.dashboard.runDetail.run-dashboard-1"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed("paged Dashboard must expose Activity")
        }
        pagedDetailButton.performClick(nil)
        let unfilteredFirstPageViews = descendants(of: pagedController.view)
        let unfilteredFirstPageCopy = unfilteredFirstPageViews.compactMap {
            ($0 as? NSTextField)?.stringValue
        }
        try require(
            unfilteredFirstPageViews.contains(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.group.1.50"
            })
                && !unfilteredFirstPageViews.contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.completionSummary"
                })
                && unfilteredFirstPageCopy.contains(
                    FoundationCopy.format(
                        .runDetailGroupLoadedThrough,
                        "2026-07-28T00:00:00.000Z",
                        "2026-07-28T00:00:00.000Z",
                        FoundationCopy.text(.runDetailFilterCommand),
                        50
                    )
                ),
            "a grouped loaded tail must not claim an exact segment end"
        )
        guard
            let firstPageEvidenceToggle = unfilteredFirstPageViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.evidenceToggle.50"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed(
                "loaded first page must expose evidence disclosure"
            )
        }
        firstPageEvidenceToggle.scrollToVisible(firstPageEvidenceToggle.bounds)
        pagedWindow.makeFirstResponder(firstPageEvidenceToggle)
        firstPageEvidenceToggle.performClick(nil)
        let focusedFirstPageViews = descendants(of: pagedController.view)
        let focusedFirstPageToggle = focusedFirstPageViews.first(where: {
            $0.accessibilityIdentifier() == "flit.runDetail.evidenceToggle.50"
        }) as? NSButton
        try require(
            focusedFirstPageToggle?.title == FoundationCopy.text(.runDetailHideEvidence)
                && pagedWindow.firstResponder === focusedFirstPageToggle
                && focusedFirstPageToggle?.visibleRect.isEmpty == false,
            "late evidence disclosure must retain its viewport anchor and keyboard focus"
        )
        guard
            let pagedCategoryFilter = focusedFirstPageViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.filter"
            }) as? NSPopUpButton
        else {
            throw NativeHealthTestFailure.failed(
                "paged Run detail must expose its category filter"
            )
        }
        pagedCategoryFilter.selectItem(
            withTitle: FoundationCopy.text(.runDetailFilterLifecycle)
        )
        pagedCategoryFilter.sendAction(
            pagedCategoryFilter.action,
            to: pagedCategoryFilter.target
        )
        let filteredFirstPageViews = descendants(of: pagedController.view)
        let filteredFirstPageCopy = filteredFirstPageViews.compactMap {
            ($0 as? NSTextField)?.stringValue
        }
        try require(
            filteredFirstPageViews.contains(where: {
                $0.accessibilityIdentifier()
                    == "flit.runDetail.noMatchingLoadedEvents.lifecycle"
            })
                && filteredFirstPageCopy.contains(
                    FoundationCopy.format(
                        .runDetailNoMatchingLoadedEvents,
                        FoundationCopy.text(.runDetailFilterLifecycle)
                    )
                )
                && !filteredFirstPageCopy.contains(
                    FoundationCopy.format(
                        .runDetailNoMatchingEvents,
                        FoundationCopy.text(.runDetailFilterLifecycle)
                    )
                ),
            "a filtered non-tail page must scope its empty result to loaded activity"
        )
        guard
            let loadMore = filteredFirstPageViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.loadMore"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed(
                "a full non-tail page must expose Load more"
            )
        }
        loadMore.performClick(nil)
        let completedPageViews = descendants(of: pagedController.view)
        let completedPageFilter = completedPageViews.first(where: {
            $0.accessibilityIdentifier() == "flit.runDetail.filter"
        }) as? NSPopUpButton
        try require(
            completedPageViews.contains(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.event.51"
            })
                && !completedPageViews.contains(where: {
                    $0.accessibilityIdentifier()
                        == "flit.runDetail.noMatchingLoadedEvents.lifecycle"
                })
                && !completedPageViews.contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.loadMore"
                })
                && completedPageFilter?.titleOfSelectedItem
                    == FoundationCopy.text(.runDetailFilterLifecycle),
            "explicit next-page load must append before filtering and preserve the selection"
        )
        guard
            let completedPageCategoryFilter = completedPageViews.first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.filter"
            }) as? NSPopUpButton
        else {
            throw NativeHealthTestFailure.failed(
                "completed page must preserve its category filter"
            )
        }
        completedPageCategoryFilter.selectItem(
            withTitle: FoundationCopy.text(.runDetailFilterCommand)
        )
        completedPageCategoryFilter.sendAction(
            completedPageCategoryFilter.action,
            to: completedPageCategoryFilter.target
        )
        try require(
            descendants(of: pagedController.view).contains(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.evidence.50"
            }),
            "expanded evidence must survive filtering, pagination, and a later rerender"
        )
        let failingPageController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { pagedDashboardFixture }
            ),
            runDetailClient: RunDetailClient(
                fixtureLoader: { request in
                    guard request.afterCursor == 0 else {
                        throw NativeHealthTestFailure.failed("next page unavailable")
                    }
                    return fullPageRunDetail
                }
            )
        )
        _ = failingPageController.view
        guard
            let failingDetailButton = descendants(of: failingPageController.view).first(where: {
                $0.accessibilityIdentifier()
                    == "flit.dashboard.runDetail.run-dashboard-1"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed("failure Dashboard must expose Activity")
        }
        failingDetailButton.performClick(nil)
        guard
            let failingEvidenceToggle = descendants(of: failingPageController.view).first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.evidenceToggle.50"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed(
                "failure fixture must expose loaded evidence disclosure"
            )
        }
        failingEvidenceToggle.performClick(nil)
        guard
            let failingLoadMore = descendants(of: failingPageController.view).first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.loadMore"
            }) as? NSButton
        else {
            throw NativeHealthTestFailure.failed("failure page must expose Load more")
        }
        failingLoadMore.performClick(nil)
        try require(
            descendants(of: failingPageController.view).contains(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.evidence.50"
            }),
            "failed next page must preserve expanded accepted evidence"
        )
        guard
            let failedCategoryFilter = descendants(of: failingPageController.view).first(where: {
                $0.accessibilityIdentifier() == "flit.runDetail.filter"
            }) as? NSPopUpButton
        else {
            throw NativeHealthTestFailure.failed(
                "failed Run detail page must preserve its category filter"
            )
        }
        failedCategoryFilter.selectItem(
            withTitle: FoundationCopy.text(.runDetailFilterLifecycle)
        )
        failedCategoryFilter.sendAction(
            failedCategoryFilter.action,
            to: failedCategoryFilter.target
        )
        let failedPageViews = descendants(of: failingPageController.view)
        try require(
            !failedPageViews.contains(where: {
                $0.accessibilityIdentifier().hasPrefix("flit.runDetail.event.")
            })
                && failedPageViews.contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.loadMore"
                })
                && failedPageViews.contains(where: {
                    $0.accessibilityIdentifier() == "flit.runDetail.pageUnavailable"
                }),
            "failed next page must preserve accepted rows, retry, and failure across filtering"
        )
        let observedController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { observedDashboardFixture }
            )
        )
        _ = observedController.view
        let observedCopy = descendants(of: observedController.view)
            .compactMap { ($0 as? NSTextField)?.stringValue }
        try require(
            observedCopy.contains(
                FoundationCopy.format(.dashboardChangesObservedDuringRun, 3, 42, 7)
            ),
            "observed Dashboard counts must preserve their visible attribution"
        )
        let overflowController = FoundationViewController(
            client: client,
            dashboardClient: DashboardClient(
                fixtureLoader: { overflowDashboardFixture }
            )
        )
        _ = overflowController.view
        let overflowWindow = NSWindow(contentViewController: overflowController)
        overflowWindow.setContentSize(NSSize(width: 1_280, height: 720))
        overflowWindow.contentView?.layoutSubtreeIfNeeded()
        let overflowViews = descendants(of: overflowController.view)
        guard
            let overflowScroll = overflowViews.first(where: {
                $0.accessibilityIdentifier() == "flit.dashboard.scroll"
            }) as? NSScrollView,
            let overflowDocument = overflowScroll.documentView,
            let attentionHeading = overflowViews.first(where: {
                $0.accessibilityIdentifier() == "flit.dashboard.section.NeedsAttention"
            }),
            let attentionCard = overflowViews.first(where: {
                $0.accessibilityIdentifier()
                    == "flit.dashboard.run.\(overflowFirstRunId)"
            })
        else {
            throw NativeHealthTestFailure.failed(
                "overflow Dashboard must expose its scroll view and priority content"
            )
        }
        let initialVisibleRect = overflowScroll.contentView.bounds
        try require(
            initialVisibleRect.intersects(
                attentionHeading.convert(attentionHeading.bounds, to: overflowDocument)
            )
                && initialVisibleRect.intersects(
                    attentionCard.convert(attentionCard.bounds, to: overflowDocument)
                ),
            "overflow Dashboard must open on Needs Attention content"
        )
        let layoutWindow = NSWindow(contentViewController: controller)
        layoutWindow.minSize = NSSize(width: 720, height: 560)
        try requireFoundationLayout(
            controller,
            in: layoutWindow,
            size: NSSize(width: 1_280, height: 720),
            expectedPanelWidth: 680
        )
        try requireFoundationLayout(
            controller,
            in: layoutWindow,
            size: NSSize(width: 720, height: 560),
            expectedPanelWidth: 624
        )

        let defaultsSuite = "dev.flit.native-health.\(ProcessInfo.processInfo.processIdentifier)"
        guard let lifecycleDefaults = UserDefaults(suiteName: defaultsSuite) else {
            throw NativeHealthTestFailure.failed(
                "native lifecycle test defaults must be available"
            )
        }
        lifecycleDefaults.removePersistentDomain(forName: defaultsSuite)
        let closePreference = CloseToTrayPreference(defaults: lifecycleDefaults)
        let closePresenter = RecordingCloseToTrayAlertPresenter()
        let firstCloseDelegate = AppDelegate(
            closeToTrayPreference: closePreference,
            closeToTrayAlertPresenter: closePresenter,
            dataDirectoryProvider: { dataDirectory }
        )
        let firstCloseWindow = NSWindow()
        firstCloseWindow.orderFront(nil)
        try require(
            !firstCloseDelegate.windowShouldClose(firstCloseWindow)
                && firstCloseWindow.isVisible
                && closePresenter.contents == [.current],
            "first window close must present the exact one-time explanation before hiding"
        )
        closePresenter.acknowledge()
        try require(
            !firstCloseWindow.isVisible,
            "acknowledging the first-close explanation must hide the window"
        )
        firstCloseWindow.orderFront(nil)
        try require(
            !firstCloseDelegate.windowShouldClose(firstCloseWindow)
                && !firstCloseWindow.isVisible
                && closePresenter.contents.count == 1,
            "repeated window close must hide without repeating the explanation"
        )

        var statusOpenCount = 0
        var statusQuitCount = 0
        let statusController = ApplicationStatusItemController(
            openHandler: { statusOpenCount += 1 },
            quitHandler: { statusQuitCount += 1 }
        )
        let statusMenuItems = statusController.statusItem.menu?.items ?? []
        try require(
            statusController.statusItem.button?.title == "Flit"
                && statusController.statusItem.button?.accessibilityIdentifier()
                    == "flit.statusItem",
            "menu-bar item must expose a stable visible and accessibility identity"
        )
        try require(
            statusMenuItems.map(\.title)
                == [
                    FoundationCopy.text(.menuOpen),
                    "",
                    FoundationCopy.text(.menuQuit),
                ]
                && statusMenuItems[0].identifier?.rawValue == "flit.statusItem.open"
                && statusMenuItems[2].identifier?.rawValue == "flit.statusItem.quit",
            "menu-bar item must expose stable Open and Quit entries"
        )
        statusController.openFlit(nil)
        statusController.quitFlit(nil)
        try require(
            statusOpenCount == 1 && statusQuitCount == 1,
            "menu-bar actions must invoke exactly one lifecycle handler"
        )

        let lifecycleQuitPresenter = RecordingExplicitQuitAlertPresenter()
        let lifecycleQuitCoordinator = ExplicitQuitCoordinator(
            previewLoader: { .exact(quitImpactFixture) },
            presenter: lifecycleQuitPresenter
        )
        var applicationTerminationRequestCount = 0
        var lifecycleTerminationReplies: [Bool] = []
        let lifecycleDelegate = AppDelegate(
            closeToTrayPreference: CloseToTrayPreference(defaults: lifecycleDefaults),
            closeToTrayAlertPresenter: closePresenter,
            dataDirectoryProvider: { dataDirectory },
            explicitQuitCoordinator: lifecycleQuitCoordinator,
            applicationTerminator: { applicationTerminationRequestCount += 1 },
            terminationReplyHandler: { _, shouldTerminate in
                lifecycleTerminationReplies.append(shouldTerminate)
            }
        )
        lifecycleDelegate.applicationDidFinishLaunching(
            Notification(name: NSApplication.didFinishLaunchingNotification)
        )
        guard let retainedWindow = lifecycleDelegate.testMainWindow else {
            throw NativeHealthTestFailure.failed(
                "application launch must retain its main window"
            )
        }
        let retainedWindowIdentity = ObjectIdentifier(retainedWindow)
        let constructionCountBeforeClose = coreConstructionCount()
        try require(
            !lifecycleDelegate.applicationShouldTerminateAfterLastWindowClosed(
                NSApplication.shared
            )
                && !lifecycleDelegate.windowShouldClose(retainedWindow)
                && !retainedWindow.isVisible,
            "last-window close must hide without terminating the app-process Core"
        )
        lifecycleDelegate.testOpenFromStatusItem()
        try require(
            lifecycleDelegate.testMainWindow.map(ObjectIdentifier.init)
                == retainedWindowIdentity
                && retainedWindow.isVisible
                && coreConstructionCount() == constructionCountBeforeClose,
            "actual menu-bar Open must restore the retained window and Core"
        )
        _ = lifecycleDelegate.windowShouldClose(retainedWindow)
        try require(
            lifecycleDelegate.applicationShouldHandleReopen(
                NSApplication.shared,
                hasVisibleWindows: false
            )
                && lifecycleDelegate.testMainWindow.map(ObjectIdentifier.init)
                    == retainedWindowIdentity
                && retainedWindow.isVisible
                && coreConstructionCount() == constructionCountBeforeClose,
            "Dock reopen must restore the retained window without reconstructing Core"
        )
        lifecycleDelegate.testQuitFromStatusItem()
        try require(
            applicationTerminationRequestCount == 1
                && lifecycleQuitPresenter.contents.isEmpty,
            "actual menu-bar Quit must request application-level termination"
        )
        guard
            let mainMenuQuitItem = NSApplication.shared.mainMenu?
                .items.first?.submenu?.items.first
        else {
            throw NativeHealthTestFailure.failed(
                "application menu must expose explicit Quit"
            )
        }
        try require(
            mainMenuQuitItem.identifier?.rawValue == "flit.mainMenu.quit"
                && mainMenuQuitItem.target === lifecycleDelegate,
            "application-menu Quit must target the shared explicit Quit coordinator"
        )
        _ = mainMenuQuitItem.target?.perform(
            mainMenuQuitItem.action,
            with: mainMenuQuitItem
        )
        try require(
            applicationTerminationRequestCount == 2
                && lifecycleQuitPresenter.contents.isEmpty,
            "application-menu Quit must share the status-item system termination path"
        )
        try require(
            lifecycleDelegate.applicationShouldTerminate(NSApplication.shared)
                == .terminateLater
                && lifecycleDelegate.applicationShouldTerminate(NSApplication.shared)
                    == .terminateLater
                && lifecycleQuitPresenter.contents == [exactQuitContent]
                && lifecycleTerminationReplies.isEmpty,
            "system termination must present one active-Run confirmation and defer its reply"
        )
        lifecycleQuitPresenter.choose(.cancel)
        try require(
            lifecycleTerminationReplies == [false],
            "cancelled system termination must reply false"
        )
        try require(
            lifecycleDelegate.applicationShouldTerminate(NSApplication.shared)
                == .terminateLater,
            "a new system termination request must reopen exact confirmation"
        )
        lifecycleQuitPresenter.choose(.confirm)
        try require(
            lifecycleTerminationReplies == [false, true],
            "confirmed unchanged system termination must reply true"
        )

        let unavailableDelegatePresenter = RecordingExplicitQuitAlertPresenter()
        var unavailableDelegateReplies: [Bool] = []
        let unavailableDelegate = AppDelegate(
            dataDirectoryProvider: { dataDirectory },
            explicitQuitCoordinator: ExplicitQuitCoordinator(
                previewLoader: { .unavailable },
                presenter: unavailableDelegatePresenter
            ),
            applicationTerminator: {},
            terminationReplyHandler: { _, shouldTerminate in
                unavailableDelegateReplies.append(shouldTerminate)
            }
        )
        try require(
            unavailableDelegate.applicationShouldTerminate(NSApplication.shared)
                == .terminateLater
                && unavailableDelegatePresenter.contents == [unavailableQuitContent],
            "system termination with unavailable impact must defer behind its warning"
        )
        unavailableDelegatePresenter.choose(.confirm)
        try require(
            unavailableDelegateReplies == [true],
            "confirmed repeated unavailable impact must complete system termination"
        )

        var changedDelegatePreviews: [ExplicitQuitPreview] = [
            .exact(quitImpactFixture),
            .exact(changedQuitImpact),
            .exact(changedQuitImpact),
        ]
        let changedDelegatePresenter = RecordingExplicitQuitAlertPresenter()
        var changedDelegateReplies: [Bool] = []
        let changedDelegate = AppDelegate(
            dataDirectoryProvider: { dataDirectory },
            explicitQuitCoordinator: ExplicitQuitCoordinator(
                previewLoader: { changedDelegatePreviews.removeFirst() },
                presenter: changedDelegatePresenter
            ),
            applicationTerminator: {},
            terminationReplyHandler: { _, shouldTerminate in
                changedDelegateReplies.append(shouldTerminate)
            }
        )
        try require(
            changedDelegate.applicationShouldTerminate(NSApplication.shared)
                == .terminateLater,
            "system termination with active Runs must defer"
        )
        changedDelegatePresenter.choose(.confirm)
        try require(
            changedDelegatePresenter.contents.count == 2
                && changedDelegateReplies.isEmpty,
            "changed system termination impact must replace its confirmation"
        )
        changedDelegatePresenter.choose(.confirm)
        try require(
            changedDelegateReplies == [true],
            "fresh confirmation of changed impact must complete system termination"
        )

        let emptyDelegatePresenter = RecordingExplicitQuitAlertPresenter()
        var emptyDelegateReplies: [Bool] = []
        let emptyDelegate = AppDelegate(
            dataDirectoryProvider: { dataDirectory },
            explicitQuitCoordinator: ExplicitQuitCoordinator(
                previewLoader: { .exact(emptyQuitImpact) },
                presenter: emptyDelegatePresenter
            ),
            applicationTerminator: {},
            terminationReplyHandler: { _, shouldTerminate in
                emptyDelegateReplies.append(shouldTerminate)
            }
        )
        try require(
            emptyDelegate.applicationShouldTerminate(NSApplication.shared)
                == .terminateNow
                && emptyDelegatePresenter.contents.isEmpty
                && emptyDelegateReplies.isEmpty,
            "system termination with an exact empty preview must proceed immediately"
        )
        lifecycleDefaults.removePersistentDomain(forName: defaultsSuite)

        let result: [String: Any] = [
            "core_construction_count": coreConstructionCount(),
            "hosted_swiftui_leaves": controller.hostedLeafCount,
            "normal_fixture": true,
            "protocol_mismatch_fixture": true,
            "state": controller.currentState.rawValue,
        ]
        let output = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        FileHandle.standardOutput.write(output)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}
