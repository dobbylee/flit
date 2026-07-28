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
    case invalidRunVersion
    case unknownSection
}

struct DashboardPresentationState: Sendable {
    private(set) var coreInstanceId: String?
    private(set) var cursor: UInt64 = 0
    private(set) var runsById: [String: FlitDashboardRunRecord] = [:]

    mutating func apply(_ response: FlitDashboardReadResponse) throws {
        switch response {
        case let .snapshot(snapshot):
            guard
                snapshot.protocolVersion == flitClientProtocolVersion,
                snapshot.eventSchemaVersion == flitEventSchemaVersion
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
}

struct DashboardClient: Sendable {
    private let fixtureLoader: (@Sendable () throws -> FlitDashboardReadResponse)?

    init(
        fixtureLoader: (@Sendable () throws -> FlitDashboardReadResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    func loadInitial() throws -> FlitDashboardReadResponse {
        if let fixtureLoader {
            return try fixtureLoader()
        }
        let request = FlitDashboardReadRequest(
            expectedCoreInstanceId: nil,
            afterCursor: nil,
            requestedEventLimit: 50,
            clientProtocolVersion: flitClientProtocolVersion
        )
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
