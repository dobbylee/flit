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
            Set(commandErrors.map { $0.code.rawValue }).count == 7,
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
            case let .available(files, insertions, deletions) =
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
                && runDetailFixture.events[0].sourceKind == .core,
            "generated Run detail must preserve structured evidence and capability facts"
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
                && managedRunRequest.permissionModeVersion == 1,
            "generated managed Run request must preserve exact Manual mode version"
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

        let lifecycleDelegate = AppDelegate(
            closeToTrayPreference: CloseToTrayPreference(defaults: lifecycleDefaults),
            closeToTrayAlertPresenter: closePresenter,
            dataDirectoryProvider: { dataDirectory }
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
