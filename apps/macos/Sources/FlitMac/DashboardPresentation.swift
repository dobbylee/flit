import Foundation

enum DashboardSection: String, CaseIterable, Sendable {
    case needsAttention = "NeedsAttention"
    case possiblyStuck = "PossiblyStuck"
    case working = "Working"
    case finished = "Finished"

    var title: String {
        switch self {
        case .needsAttention: FoundationCopy.text(.dashboardSectionNeedsAttention)
        case .possiblyStuck: FoundationCopy.text(.dashboardSectionPossiblyStuck)
        case .working: FoundationCopy.text(.dashboardSectionWorking)
        case .finished: FoundationCopy.text(.dashboardSectionFinished)
        }
    }
}

enum DashboardPresentationError: Error, Equatable {
    case contractMismatch
    case cursorMismatch
    case duplicateRunId
    case instanceMismatch
    case invalidEventPage
    case invalidRunVersion
    case nonAdvancingPage
    case pageLimitExceeded
    case unknownSection
}

enum ActiveAttentionPresentationError: Error, Equatable {
    case contractMismatch
    case identityMismatch
    case invalidItem
}

enum ActiveAttentionPresentationAction: Equatable, Sendable {
    case acknowledge
    case stillWorking(occurrenceId: String)
    case permissionDetailsUnavailable
    case unavailable
}

struct ActiveAttentionCardPresentation: Equatable, Sendable {
    let attentionId: String
    let attentionVersion: UInt64
    let category: FlitRunActiveAttentionCategory
    let severity: FlitRunActiveAttentionSeverity
    let status: FlitRunActiveAttentionStatus
    let blocking: Bool
    let sourceEventType: String
    let sourceObservedAt: String
    let contentUnavailableReason: String
    let action: ActiveAttentionPresentationAction
}

func activeAttentionCard(
    from response: FlitRunActiveAttentionReadResponse,
    for run: FlitDashboardRunRecord
) throws -> ActiveAttentionCardPresentation? {
    guard
        response.protocolVersion == flitClientProtocolVersion,
        response.eventSchemaVersion == flitEventSchemaVersion
    else {
        throw ActiveAttentionPresentationError.contractMismatch
    }
    guard
        response.runId == run.runId,
        response.runVersion == run.version,
        response.openCount == run.attentionOpenCount
    else {
        throw ActiveAttentionPresentationError.identityMismatch
    }
    switch response.item {
    case .null:
        guard response.openCount == 0, run.attentionLevel == "None" else {
            throw ActiveAttentionPresentationError.invalidItem
        }
        return nil
    case let .item(item):
        guard
            response.openCount > 0,
            item.attentionVersion > 0,
            item.attentionVersion <= run.version,
            boundedAttentionToken(item.attentionId, maximumBytes: 256),
            boundedAttentionToken(item.sourceEventId, maximumBytes: 256),
            boundedAttentionToken(item.sourceEventType, maximumBytes: 256),
            boundedAttentionToken(item.sourceObservedAt, maximumBytes: 256),
            boundedAttentionToken(item.contentUnavailableReason, maximumBytes: 4 * 1_024),
            run.attentionLevel == attentionSeverityCoreName(item.severity)
        else {
            throw ActiveAttentionPresentationError.invalidItem
        }
        let action: ActiveAttentionPresentationAction
        switch item.action {
        case .acknowledge:
            guard
                item.category == .failure,
                item.status == .open,
                !item.blocking,
                ["run.failed", "run.interrupted", "run.resume_failed"]
                .contains(item.sourceEventType)
            else {
                throw ActiveAttentionPresentationError.invalidItem
            }
            action = .acknowledge
        case let .permissionResponse(requestId, requestVersion):
            guard
                item.category == .permission,
                item.status == .open,
                item.blocking,
                requestVersion == item.attentionVersion,
                boundedAttentionToken(requestId, maximumBytes: 256)
            else {
                throw ActiveAttentionPresentationError.invalidItem
            }
            // The current provider contract omits the command, cwd, affected paths, and raw
            // request text required by the permission-card safety contract. Preserve the exact
            // identity without presenting an approval control until those facts are available.
            action = .permissionDetailsUnavailable
        case let .stillWorking(occurrenceId):
            guard
                item.category == .stuck,
                item.status == .open,
                boundedAttentionToken(occurrenceId, maximumBytes: 256),
                run.activeStuckOccurrenceId == occurrenceId
            else {
                throw ActiveAttentionPresentationError.invalidItem
            }
            action = .stillWorking(occurrenceId: occurrenceId)
        case let .unavailable(reason):
            guard boundedAttentionToken(reason, maximumBytes: 4 * 1_024) else {
                throw ActiveAttentionPresentationError.invalidItem
            }
            action = .unavailable
        }
        return ActiveAttentionCardPresentation(
            attentionId: item.attentionId,
            attentionVersion: item.attentionVersion,
            category: item.category,
            severity: item.severity,
            status: item.status,
            blocking: item.blocking,
            sourceEventType: item.sourceEventType,
            sourceObservedAt: item.sourceObservedAt,
            contentUnavailableReason: item.contentUnavailableReason,
            action: action
        )
    }
}

enum AttentionAcknowledgeClientError: Error, Equatable {
    case contractMismatch
    case identityMismatch
    case invalidResponse
}

@MainActor
struct AttentionAcknowledgeClient {
    private let fixtureLoader:
        ((FlitAttentionAcknowledgeRequest) throws
            -> FlitAttentionAcknowledgeResponse)?

    init(
        fixtureLoader: ((FlitAttentionAcknowledgeRequest) throws
            -> FlitAttentionAcknowledgeResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    func submit(
        runId: String,
        expectedRunVersion: UInt64,
        attentionId: String,
        attentionVersion: UInt64
    ) throws -> FlitAttentionAcknowledgeResponse {
        let request = FlitAttentionAcknowledgeRequest(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            attentionId: attentionId,
            attentionVersion: attentionVersion,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitAttentionAcknowledgeResponse
        if let fixtureLoader {
            response = try fixtureLoader(request)
        } else {
            let requestData = try JSONEncoder().encode(request)
            let rendered = try attentionAcknowledgeJson(
                requestJson: String(decoding: requestData, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitAttentionAcknowledgeResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard response.protocolVersion == flitClientProtocolVersion else {
            throw AttentionAcknowledgeClientError.contractMismatch
        }
        guard
            response.runId == runId,
            response.attentionId == attentionId,
            response.attentionVersion == attentionVersion
        else {
            throw AttentionAcknowledgeClientError.identityMismatch
        }
        switch response.status {
        case .applied:
            guard
                response.previousVersion == expectedRunVersion,
                response.eventVersion.map({ $0 > expectedRunVersion }) == true,
                response.eventId.map({ boundedAttentionToken($0, maximumBytes: 256) }) == true,
                response.expectedRunVersion == nil,
                response.reason == nil
            else {
                throw AttentionAcknowledgeClientError.invalidResponse
            }
        case .rejected:
            guard
                response.expectedRunVersion == expectedRunVersion,
                response.previousVersion == nil,
                response.eventId == nil,
                response.eventVersion == nil,
                response.reason != nil
            else {
                throw AttentionAcknowledgeClientError.invalidResponse
            }
        }
        return response
    }
}

private func boundedAttentionToken(_ value: String, maximumBytes: Int) -> Bool {
    !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        && value.utf8.count <= maximumBytes
        && !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
}

private func attentionSeverityCoreName(
    _ severity: FlitRunActiveAttentionSeverity
) -> String {
    switch severity {
    case .informational: "Informational"
    case .actionRequired: "ActionRequired"
    case .critical: "Critical"
    }
}

@MainActor
struct ActiveAttentionClient {
    private let fixtureLoader:
        ((FlitRunActiveAttentionReadRequest) throws
            -> FlitRunActiveAttentionReadResponse)?

    init(
        fixtureLoader: ((FlitRunActiveAttentionReadRequest) throws
            -> FlitRunActiveAttentionReadResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    func load(for run: FlitDashboardRunRecord) throws -> ActiveAttentionCardPresentation? {
        let request = FlitRunActiveAttentionReadRequest(
            runId: run.runId,
            expectedRunVersion: run.version,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitRunActiveAttentionReadResponse
        if let fixtureLoader {
            response = try fixtureLoader(request)
        } else {
            let requestData = try JSONEncoder().encode(request)
            let rendered = try runActiveAttentionReadJson(
                requestJson: String(decoding: requestData, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitRunActiveAttentionReadResponse.self,
                from: Data(rendered.utf8)
            )
        }
        return try activeAttentionCard(from: response, for: run)
    }
}

struct DashboardPresentationState: Sendable {
    private static let maximumDeltaEvents = 50

    private(set) var coreInstanceId: String?
    private(set) var cursor: UInt64 = 0
    private(set) var runsById: [String: FlitDashboardRunRecord] = [:]

    mutating func apply(_ response: FlitDashboardReadResponse) throws {
        switch response {
        case let .snapshot(snapshot):
            guard
                snapshot.protocolVersion == flitClientProtocolVersion,
                snapshot.eventSchemaVersion == flitEventSchemaVersion,
                !snapshot.hasMore
            else {
                throw DashboardPresentationError.contractMismatch
            }
            let runs = try uniqueRuns(snapshot.runs)
            guard runs.values.allSatisfy({ $0.version <= snapshot.nextCursor }) else {
                throw DashboardPresentationError.invalidRunVersion
            }
            coreInstanceId = snapshot.coreInstanceId
            cursor = snapshot.nextCursor
            runsById = runs
        case let .delta(delta):
            guard
                delta.protocolVersion == flitClientProtocolVersion,
                delta.eventSchemaVersion == flitEventSchemaVersion
            else {
                throw DashboardPresentationError.contractMismatch
            }
            guard coreInstanceId == delta.coreInstanceId else {
                throw DashboardPresentationError.instanceMismatch
            }
            guard delta.requestedAfterCursor == cursor, delta.nextCursor >= cursor else {
                throw DashboardPresentationError.cursorMismatch
            }
            try validateEventPage(delta)
            let upserts = try uniqueRuns(delta.runs)
            guard upserts.values.allSatisfy({
                $0.version > delta.requestedAfterCursor && $0.version <= delta.nextCursor
            }) else {
                throw DashboardPresentationError.invalidRunVersion
            }
            for run in upserts.values {
                runsById[run.runId] = run
            }
            cursor = delta.nextCursor
        }
    }

    func runs(in section: DashboardSection) throws -> [FlitDashboardRunRecord] {
        try runsById.values
            .filter { run in
                guard DashboardSection(rawValue: run.dashboardBucket) != nil else {
                    throw DashboardPresentationError.unknownSection
                }
                return run.dashboardBucket == section.rawValue
            }
            .sorted { $0.runId < $1.runId }
    }

    private func uniqueRuns(
        _ runs: [FlitDashboardRunRecord]
    ) throws -> [String: FlitDashboardRunRecord] {
        var result: [String: FlitDashboardRunRecord] = [:]
        for run in runs {
            guard result.updateValue(run, forKey: run.runId) == nil else {
                throw DashboardPresentationError.duplicateRunId
            }
            guard DashboardSection(rawValue: run.dashboardBucket) != nil else {
                throw DashboardPresentationError.unknownSection
            }
        }
        return result
    }

    private func validateEventPage(
        _ delta: FlitDashboardDeltaResponse
    ) throws {
        guard
            delta.events.count <= Self.maximumDeltaEvents,
            delta.retainedAfterCursor <= delta.requestedAfterCursor
        else {
            throw DashboardPresentationError.invalidEventPage
        }
        guard !delta.events.isEmpty else {
            guard
                delta.nextCursor == delta.requestedAfterCursor,
                !delta.hasMore
            else {
                if delta.hasMore && delta.nextCursor == delta.requestedAfterCursor {
                    throw DashboardPresentationError.nonAdvancingPage
                }
                throw DashboardPresentationError.invalidEventPage
            }
            return
        }

        var previousCursor = delta.requestedAfterCursor
        for event in delta.events {
            guard
                event.cursor > previousCursor,
                event.cursor <= delta.nextCursor
            else {
                throw DashboardPresentationError.invalidEventPage
            }
            previousCursor = event.cursor
        }
        guard previousCursor == delta.nextCursor else {
            throw DashboardPresentationError.invalidEventPage
        }
        if delta.hasMore {
            guard
                delta.events.count == Self.maximumDeltaEvents,
                delta.nextCursor > delta.requestedAfterCursor
            else {
                throw DashboardPresentationError.invalidEventPage
            }
        }
    }
}

@MainActor
struct DashboardClient {
    static let requestedEventLimit: UInt32 = 50
    static let maximumConvergencePages = 40

    private let requestLoader:
        ((FlitDashboardReadRequest) throws -> FlitDashboardReadResponse)?

    init(
        fixtureLoader: (() throws -> FlitDashboardReadResponse)? = nil
    ) {
        requestLoader = fixtureLoader.map { fixtureLoader in
            { _ in try fixtureLoader() }
        }
    }

    init(
        requestLoader: @escaping (FlitDashboardReadRequest) throws
            -> FlitDashboardReadResponse
    ) {
        self.requestLoader = requestLoader
    }

    func loadInitial() throws -> FlitDashboardReadResponse {
        let request = FlitDashboardReadRequest(
            expectedCoreInstanceId: nil,
            afterCursor: nil,
            requestedEventLimit: Self.requestedEventLimit,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response = try load(request)
        guard case let .snapshot(snapshot) = response else {
            throw DashboardPresentationError.contractMismatch
        }
        guard
            snapshot.reason == .initial,
            snapshot.requestedAfterCursor == nil,
            snapshot.retainedAfterCursor <= snapshot.nextCursor
        else {
            throw DashboardPresentationError.contractMismatch
        }
        return response
    }

    func convergedState(
        from accepted: DashboardPresentationState
    ) throws -> DashboardPresentationState {
        guard let initialCoreInstanceId = accepted.coreInstanceId else {
            throw DashboardPresentationError.instanceMismatch
        }
        var candidate = accepted
        var coreInstanceId = initialCoreInstanceId
        for _ in 0..<Self.maximumConvergencePages {
            let requestedCursor = candidate.cursor
            let response = try load(
                FlitDashboardReadRequest(
                    expectedCoreInstanceId: coreInstanceId,
                    afterCursor: requestedCursor,
                    requestedEventLimit: Self.requestedEventLimit,
                    clientProtocolVersion: flitClientProtocolVersion
                )
            )
            if case let .snapshot(snapshot) = response {
                try validateResyncSnapshot(
                    snapshot,
                    expectedCoreInstanceId: coreInstanceId,
                    requestedCursor: requestedCursor
                )
            }
            try candidate.apply(response)
            switch response {
            case .snapshot:
                return candidate
            case let .delta(delta):
                if !delta.hasMore {
                    return candidate
                }
                guard candidate.cursor > requestedCursor else {
                    throw DashboardPresentationError.nonAdvancingPage
                }
                coreInstanceId = candidate.coreInstanceId ?? coreInstanceId
            }
        }
        throw DashboardPresentationError.pageLimitExceeded
    }

    private func validateResyncSnapshot(
        _ snapshot: FlitDashboardSnapshotResponse,
        expectedCoreInstanceId: String,
        requestedCursor: UInt64
    ) throws {
        guard
            snapshot.requestedAfterCursor == requestedCursor,
            snapshot.retainedAfterCursor <= snapshot.nextCursor
        else {
            throw DashboardPresentationError.contractMismatch
        }
        switch snapshot.reason {
        case .initial:
            throw DashboardPresentationError.contractMismatch
        case .coreInstanceMismatch:
            guard snapshot.coreInstanceId != expectedCoreInstanceId else {
                throw DashboardPresentationError.contractMismatch
            }
        case .cursorAhead:
            guard
                snapshot.coreInstanceId == expectedCoreInstanceId,
                requestedCursor > snapshot.nextCursor
            else {
                throw DashboardPresentationError.contractMismatch
            }
        case .cursorExpired:
            guard
                snapshot.coreInstanceId == expectedCoreInstanceId,
                requestedCursor < snapshot.retainedAfterCursor
            else {
                throw DashboardPresentationError.contractMismatch
            }
        }
    }

    private func load(
        _ request: FlitDashboardReadRequest
    ) throws -> FlitDashboardReadResponse {
        if let requestLoader {
            return try requestLoader(request)
        }
        let requestData = try JSONEncoder().encode(request)
        let rendered = try dashboardReadJson(
            requestJson: String(decoding: requestData, as: UTF8.self)
        )
        return try JSONDecoder().decode(
            FlitDashboardReadResponse.self,
            from: Data(rendered.utf8)
        )
    }
}

enum StuckMonitoringClientError: Error, Equatable {
    case contractMismatch
    case identityMismatch
    case invalidResponse
}

@MainActor
struct StuckAssessmentClient {
    static let maximumAssessedRuns: UInt32 = 100

    private let fixtureLoader:
        ((FlitManagedRunsAssessStuckRequest) throws
            -> FlitManagedRunsAssessStuckResponse)?

    init(
        fixtureLoader: ((FlitManagedRunsAssessStuckRequest) throws
            -> FlitManagedRunsAssessStuckResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    @discardableResult
    func assess() throws -> FlitManagedRunsAssessStuckResponse {
        let request = FlitManagedRunsAssessStuckRequest(
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitManagedRunsAssessStuckResponse
        if let fixtureLoader {
            response = try fixtureLoader(request)
        } else {
            let requestData = try JSONEncoder().encode(request)
            let rendered = try managedRunsAssessStuckJson(
                requestJson: String(decoding: requestData, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitManagedRunsAssessStuckResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard response.protocolVersion == flitClientProtocolVersion else {
            throw StuckMonitoringClientError.contractMismatch
        }
        guard
            response.assessedRuns <= Self.maximumAssessedRuns,
            response.transitionsAppended <= response.assessedRuns,
            response.unchangedRuns <= response.assessedRuns,
            UInt64(response.transitionsAppended) + UInt64(response.unchangedRuns)
                == UInt64(response.assessedRuns),
            response.unavailableRuns <= response.unchangedRuns
        else {
            throw StuckMonitoringClientError.invalidResponse
        }
        return response
    }
}

@MainActor
struct StillWorkingClient {
    private let fixtureLoader:
        ((FlitManagedRunStillWorkingRequest) throws
            -> FlitManagedRunStillWorkingResponse)?

    init(
        fixtureLoader: ((FlitManagedRunStillWorkingRequest) throws
            -> FlitManagedRunStillWorkingResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    func submit(
        runId: String,
        expectedRunVersion: UInt64,
        occurrenceId: String
    ) throws -> FlitManagedRunStillWorkingResponse {
        let request = FlitManagedRunStillWorkingRequest(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            occurrenceId: occurrenceId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitManagedRunStillWorkingResponse
        if let fixtureLoader {
            response = try fixtureLoader(request)
        } else {
            let requestData = try JSONEncoder().encode(request)
            let rendered = try managedRunStillWorkingJson(
                requestJson: String(decoding: requestData, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitManagedRunStillWorkingResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard response.protocolVersion == flitClientProtocolVersion else {
            throw StuckMonitoringClientError.contractMismatch
        }
        guard response.runId == runId, response.occurrenceId == occurrenceId else {
            throw StuckMonitoringClientError.identityMismatch
        }
        switch response.status {
        case .applied:
            guard
                response.previousVersion == expectedRunVersion,
                response.eventVersion.map({ $0 > expectedRunVersion }) == true,
                response.eventId?.isEmpty == false
            else {
                throw StuckMonitoringClientError.invalidResponse
            }
        case .rejected:
            guard response.expectedRunVersion == expectedRunVersion else {
                throw StuckMonitoringClientError.invalidResponse
            }
        }
        return response
    }
}

@MainActor
protocol DashboardCadenceScheduling: AnyObject {
    func start(_ tick: @escaping @MainActor @Sendable () -> Void)
    func stop()
}

@MainActor
final class InactiveDashboardCadence: DashboardCadenceScheduling {
    func start(_ tick: @escaping @MainActor @Sendable () -> Void) {}
    func stop() {}
}

@MainActor
final class TimerDashboardCadence: DashboardCadenceScheduling {
    private static let interval: TimeInterval = 5
    nonisolated(unsafe) private var timer: Timer?

    func start(_ tick: @escaping @MainActor @Sendable () -> Void) {
        guard timer == nil else { return }
        let timer = Timer(timeInterval: Self.interval, repeats: true) { _ in
            MainActor.assumeIsolated {
                tick()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    deinit {
        timer?.invalidate()
    }
}

@MainActor
enum DashboardCadenceFactory {
    static func makeDefault() -> any DashboardCadenceScheduling {
        #if FLIT_NATIVE_TESTS
            InactiveDashboardCadence()
        #else
            TimerDashboardCadence()
        #endif
    }
}
