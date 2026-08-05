import Foundation

enum RunCompletionSummaryError: Error, Equatable {
    case invalidProjection
}

struct RunCompletionSummary: Sendable {
    let result: String
    let projectDisplayName: String
    let provider: FlitProviderKind
    let startedAt: String?
    let endedAt: String
    let changes: FlitDashboardChangeSummary
}

func runCompletionSummary(
    for run: FlitDashboardRunRecord
) throws -> RunCompletionSummary? {
    guard
        !run.runId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
        !run.projectDisplayName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
        run.startedAt.map({
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }) ?? true
    else {
        throw RunCompletionSummaryError.invalidProjection
    }
    if case let .unavailable(reason) = run.changes,
        reason.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    {
        throw RunCompletionSummaryError.invalidProjection
    }

    switch run.lifecycle {
    case "Starting", "Running":
        guard run.endedAt == nil else {
            throw RunCompletionSummaryError.invalidProjection
        }
        return nil
    case "Finished", "Failed", "Stopped", "Interrupted":
        guard
            let endedAt = run.endedAt,
            !endedAt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            throw RunCompletionSummaryError.invalidProjection
        }
        return RunCompletionSummary(
            result: run.lifecycle,
            projectDisplayName: run.projectDisplayName,
            provider: run.provider,
            startedAt: run.startedAt,
            endedAt: endedAt,
            changes: run.changes
        )
    default:
        throw RunCompletionSummaryError.invalidProjection
    }
}

enum RunDetailPresentationError: Error, Equatable {
    case contractMismatch
    case runIdentityMismatch
    case runVersionMismatch
    case cursorMismatch
    case invalidEvent
    case duplicateEvent
}

struct RunActivityRow: Sendable {
    let cursor: UInt64
    let eventId: String
    let eventType: String
    let category: FlitRunEvidenceCategory
    let sourceKind: FlitEventSourceKind
    let confidence: Double
    let observedAt: String
}

struct RunActivityGroup: Sendable {
    let category: FlitRunEvidenceCategory
    private(set) var events: [RunActivityRow]

    init(event: RunActivityRow) {
        category = event.category
        events = [event]
    }

    var startedAt: String {
        events[0].observedAt
    }

    var endedAt: String {
        events[events.count - 1].observedAt
    }

    mutating func append(_ event: RunActivityRow) {
        precondition(category != .unknown && event.category == category)
        events.append(event)
    }
}

enum RunActivityFilter: CaseIterable, Equatable, Sendable {
    case all
    case activity
    case command
    case file
    case test
    case attention
    case lifecycle

    func visibleRows(in rows: [RunActivityRow]) -> [RunActivityRow] {
        guard let category else { return rows }
        return rows.filter { $0.category == category }
    }

    func visibleGroups(in rows: [RunActivityRow]) -> [RunActivityGroup] {
        var groups: [RunActivityGroup] = []
        for row in rows {
            if row.category != .unknown, groups.last?.category == row.category {
                groups[groups.count - 1].append(row)
            } else {
                groups.append(RunActivityGroup(event: row))
            }
        }
        guard let category else { return groups }
        return groups.filter { $0.category == category }
    }

    private var category: FlitRunEvidenceCategory? {
        switch self {
        case .all: nil
        case .activity: .activity
        case .command: .command
        case .file: .file
        case .test: .test
        case .attention: .attention
        case .lifecycle: .lifecycle
        }
    }
}

struct RunDetailPresentationState: Sendable {
    private(set) var runId: String?
    private(set) var runVersion: UInt64?
    private(set) var nextCursor: UInt64 = 0
    private(set) var hasMore = false
    private(set) var historyStatus: FlitCapabilityStatus?
    private(set) var openInProviderStatus: FlitCapabilityStatus?
    private(set) var events: [RunActivityRow] = []

    mutating func apply(
        _ response: FlitRunDetailReadResponse,
        requestedRunId: String,
        expectedRunVersion: UInt64,
        requestedAfterCursor: UInt64,
        requestedEventLimit: UInt32
    ) throws {
        let rows = try validatedRows(
            response,
            requestedRunId: requestedRunId,
            expectedRunVersion: expectedRunVersion,
            requestedAfterCursor: requestedAfterCursor,
            requestedEventLimit: requestedEventLimit
        )

        runId = response.runId
        runVersion = response.runVersion
        nextCursor = response.nextCursor
        hasMore = response.hasMore
        historyStatus = response.historyStatus
        openInProviderStatus = response.openInProviderStatus
        events = rows
    }

    mutating func append(
        _ response: FlitRunDetailReadResponse,
        requestedRunId: String,
        expectedRunVersion: UInt64,
        requestedAfterCursor: UInt64,
        requestedEventLimit: UInt32
    ) throws {
        guard runId == requestedRunId else {
            throw RunDetailPresentationError.runIdentityMismatch
        }
        guard runVersion == expectedRunVersion else {
            throw RunDetailPresentationError.runVersionMismatch
        }
        guard hasMore, nextCursor == requestedAfterCursor else {
            throw RunDetailPresentationError.cursorMismatch
        }
        guard
            historyStatus?.rawValue == response.historyStatus.rawValue,
            openInProviderStatus?.rawValue == response.openInProviderStatus.rawValue
        else {
            throw RunDetailPresentationError.contractMismatch
        }
        let rows = try validatedRows(
            response,
            requestedRunId: requestedRunId,
            expectedRunVersion: expectedRunVersion,
            requestedAfterCursor: requestedAfterCursor,
            requestedEventLimit: requestedEventLimit
        )
        let acceptedEventIds = Set(events.map(\.eventId))
        guard rows.allSatisfy({ !acceptedEventIds.contains($0.eventId) }) else {
            throw RunDetailPresentationError.duplicateEvent
        }

        nextCursor = response.nextCursor
        hasMore = response.hasMore
        historyStatus = response.historyStatus
        openInProviderStatus = response.openInProviderStatus
        events.append(contentsOf: rows)
    }
}

private func validatedRows(
    _ response: FlitRunDetailReadResponse,
    requestedRunId: String,
    expectedRunVersion: UInt64,
    requestedAfterCursor: UInt64,
    requestedEventLimit: UInt32
) throws -> [RunActivityRow] {
    guard
        response.protocolVersion == flitClientProtocolVersion,
        response.eventSchemaVersion == flitEventSchemaVersion
    else {
        throw RunDetailPresentationError.contractMismatch
    }
    guard response.runId == requestedRunId else {
        throw RunDetailPresentationError.runIdentityMismatch
    }
    guard response.runVersion == expectedRunVersion else {
        throw RunDetailPresentationError.runVersionMismatch
    }
    guard
        response.nextCursor >= requestedAfterCursor,
        response.nextCursor <= response.runVersion,
        (1 ... 50).contains(requestedEventLimit),
        response.events.count <= Int(requestedEventLimit),
        response.hasMore == (response.nextCursor < response.runVersion),
        !response.hasMore || response.events.count == Int(requestedEventLimit)
    else {
        throw RunDetailPresentationError.cursorMismatch
    }

    var previousCursor = requestedAfterCursor
    var eventIds = Set<String>()
    var rows: [RunActivityRow] = []
    rows.reserveCapacity(response.events.count)
    for event in response.events {
        guard
            event.cursor > previousCursor,
            event.cursor <= response.nextCursor,
            !event.eventId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            !event.eventType.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            !event.observedAt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            event.confidence.isFinite,
            (0.0 ... 1.0).contains(event.confidence)
        else {
            throw RunDetailPresentationError.invalidEvent
        }
        guard eventIds.insert(event.eventId).inserted else {
            throw RunDetailPresentationError.duplicateEvent
        }
        rows.append(
            RunActivityRow(
                cursor: event.cursor,
                eventId: event.eventId,
                eventType: event.eventType,
                category: event.category,
                sourceKind: event.sourceKind,
                confidence: event.confidence,
                observedAt: event.observedAt
            )
        )
        previousCursor = event.cursor
    }
    guard
        response.events.isEmpty
            ? response.nextCursor == requestedAfterCursor && !response.hasMore
            : response.nextCursor == previousCursor
    else {
        throw RunDetailPresentationError.cursorMismatch
    }
    return rows
}

struct RunDetailClient: Sendable {
    private let fixtureLoader:
        (@Sendable (FlitRunDetailReadRequest) throws -> FlitRunDetailReadResponse)?

    init(
        fixtureLoader:
            (@Sendable (FlitRunDetailReadRequest) throws -> FlitRunDetailReadResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    func loadPage(
        runId: String,
        expectedRunVersion: UInt64,
        afterCursor: UInt64
    ) throws -> FlitRunDetailReadResponse {
        let request = FlitRunDetailReadRequest(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            afterCursor: afterCursor,
            requestedEventLimit: 50,
            clientProtocolVersion: flitClientProtocolVersion
        )
        if let fixtureLoader {
            return try fixtureLoader(request)
        }
        let requestData = try JSONEncoder().encode(request)
        let rendered = try runDetailReadJson(
            requestJson: String(decoding: requestData, as: UTF8.self)
        )
        return try JSONDecoder().decode(
            FlitRunDetailReadResponse.self,
            from: Data(rendered.utf8)
        )
    }

    func loadFirstPage(
        runId: String,
        expectedRunVersion: UInt64
    ) throws -> FlitRunDetailReadResponse {
        try loadPage(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            afterCursor: 0
        )
    }
}
