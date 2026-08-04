import Foundation

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
    let sourceKind: FlitEventSourceKind
    let confidence: Double
    let observedAt: String
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

        runId = response.runId
        runVersion = response.runVersion
        nextCursor = response.nextCursor
        hasMore = response.hasMore
        historyStatus = response.historyStatus
        openInProviderStatus = response.openInProviderStatus
        events = rows
    }
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

    func loadFirstPage(
        runId: String,
        expectedRunVersion: UInt64
    ) throws -> FlitRunDetailReadResponse {
        let request = FlitRunDetailReadRequest(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            afterCursor: 0,
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
}
