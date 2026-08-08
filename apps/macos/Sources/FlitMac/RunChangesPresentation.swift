import Foundation

enum RunChangesPresentationError: Error, Equatable {
    case contractMismatch
    case runIdentityMismatch
    case runVersionMismatch
    case cursorMismatch
    case metadataMismatch
    case invalidChange
    case duplicateChange
}

enum RunChangeHeadPresentation: Equatable, Sendable {
    case available(String)
    case unavailable
}

enum RunChangesPresentationAvailability: Sendable {
    case available(
        attribution: FlitDashboardChangeAttribution,
        baselineHead: RunChangeHeadPresentation,
        terminalHead: RunChangeHeadPresentation
    )
    case unavailable(FlitRunChangesUnavailableReason)
}

struct RunChangeRow: Sendable {
    let changeId: String
    let displayPath: String
    let status: FlitRunFileChangeStatus
    let committed: Bool
    let staged: Bool
    let unstaged: Bool
    let binary: Bool
    let insertions: UInt64?
    let deletions: UInt64?
    let projectScope: FlitRunFileProjectScope
}

struct RunChangesPresentationState: Sendable {
    private(set) var runId: String?
    private(set) var runVersion: UInt64?
    private(set) var nextCursor: String?
    private(set) var hasMore = false
    private(set) var availability: RunChangesPresentationAvailability?
    private(set) var changes: [RunChangeRow] = []

    mutating func apply(
        _ response: FlitRunChangesReadResponse,
        requestedRunId: String,
        expectedRunVersion: UInt64,
        requestedAfterCursor: String?,
        requestedChangeLimit: UInt32
    ) throws {
        switch response {
        case let .available(response):
            let page = try validatedChangesPage(
                response,
                requestedRunId: requestedRunId,
                expectedRunVersion: expectedRunVersion,
                requestedAfterCursor: requestedAfterCursor,
                requestedChangeLimit: requestedChangeLimit
            )
            runId = response.runId
            runVersion = response.runVersion
            nextCursor = response.nextCursor
            hasMore = response.hasMore
            availability = .available(
                attribution: response.attribution,
                baselineHead: page.baselineHead,
                terminalHead: page.terminalHead
            )
            changes = page.rows
        case let .unavailable(response):
            try validateChangesResponseIdentity(
                protocolVersion: response.protocolVersion,
                runId: response.runId,
                runVersion: response.runVersion,
                requestedRunId: requestedRunId,
                expectedRunVersion: expectedRunVersion
            )
            guard
                response.availability == .unavailable,
                requestedAfterCursor == nil,
                (1 ... 50).contains(requestedChangeLimit)
            else {
                throw RunChangesPresentationError.cursorMismatch
            }
            runId = response.runId
            runVersion = response.runVersion
            nextCursor = nil
            hasMore = false
            availability = .unavailable(response.reason)
            changes = []
        }
    }

    mutating func append(
        _ response: FlitRunChangesReadResponse,
        requestedRunId: String,
        expectedRunVersion: UInt64,
        requestedAfterCursor: String,
        requestedChangeLimit: UInt32
    ) throws {
        guard runId == requestedRunId else {
            throw RunChangesPresentationError.runIdentityMismatch
        }
        guard runVersion == expectedRunVersion else {
            throw RunChangesPresentationError.runVersionMismatch
        }
        guard hasMore, nextCursor == requestedAfterCursor else {
            throw RunChangesPresentationError.cursorMismatch
        }
        guard
            case let .available(
                currentAttribution,
                currentBaselineHead,
                currentTerminalHead
            ) = availability,
            case let .available(response) = response
        else {
            throw RunChangesPresentationError.metadataMismatch
        }
        let page = try validatedChangesPage(
            response,
            requestedRunId: requestedRunId,
            expectedRunVersion: expectedRunVersion,
            requestedAfterCursor: requestedAfterCursor,
            requestedChangeLimit: requestedChangeLimit
        )
        guard !page.rows.isEmpty else {
            throw RunChangesPresentationError.cursorMismatch
        }
        guard
            currentAttribution == response.attribution,
            currentBaselineHead == page.baselineHead,
            currentTerminalHead == page.terminalHead
        else {
            throw RunChangesPresentationError.metadataMismatch
        }
        let acceptedIds = Set(changes.map(\.changeId))
        guard page.rows.allSatisfy({ !acceptedIds.contains($0.changeId) }) else {
            throw RunChangesPresentationError.duplicateChange
        }

        nextCursor = response.nextCursor
        hasMore = response.hasMore
        changes.append(contentsOf: page.rows)
    }
}

private struct ValidatedChangesPage {
    let baselineHead: RunChangeHeadPresentation
    let terminalHead: RunChangeHeadPresentation
    let rows: [RunChangeRow]
}

private func validateChangesResponseIdentity(
    protocolVersion: String,
    runId: String,
    runVersion: UInt64,
    requestedRunId: String,
    expectedRunVersion: UInt64
) throws {
    guard protocolVersion == flitClientProtocolVersion else {
        throw RunChangesPresentationError.contractMismatch
    }
    guard runId == requestedRunId else {
        throw RunChangesPresentationError.runIdentityMismatch
    }
    guard runVersion == expectedRunVersion else {
        throw RunChangesPresentationError.runVersionMismatch
    }
}

private func validatedChangesPage(
    _ response: FlitRunChangesAvailableResponse,
    requestedRunId: String,
    expectedRunVersion: UInt64,
    requestedAfterCursor: String?,
    requestedChangeLimit: UInt32
) throws -> ValidatedChangesPage {
    try validateChangesResponseIdentity(
        protocolVersion: response.protocolVersion,
        runId: response.runId,
        runVersion: response.runVersion,
        requestedRunId: requestedRunId,
        expectedRunVersion: expectedRunVersion
    )
    guard
        response.availability == .available,
        (1 ... 50).contains(requestedChangeLimit),
        response.changes.count <= Int(requestedChangeLimit),
        !response.hasMore || response.changes.count == Int(requestedChangeLimit)
    else {
        throw RunChangesPresentationError.cursorMismatch
    }
    let baselineHead = try validatedChangeHead(response.baselineHead)
    let terminalHead = try validatedChangeHead(response.terminalHead)
    if response.attribution == .exact {
        guard
            case .available = baselineHead,
            case .available = terminalHead
        else {
            throw RunChangesPresentationError.metadataMismatch
        }
    }

    var changeIds = Set<String>()
    var rows: [RunChangeRow] = []
    rows.reserveCapacity(response.changes.count)
    for change in response.changes {
        guard
            validOpaqueChangeId(change.changeId),
            change.changeId != requestedAfterCursor,
            changeIds.insert(change.changeId).inserted
        else {
            throw RunChangesPresentationError.duplicateChange
        }
        guard
            !change.displayPath.isEmpty,
            change.displayPath.utf8.count <= 49_152,
            !change.displayPath.contains("\0"),
            change.committed || change.staged || change.unstaged,
            change.insertions.isSome == change.deletions.isSome,
            !change.binary || change.insertions == nil,
            change.insertions.map({ $0 <= 9_007_199_254_740_991 }) ?? true,
            change.deletions.map({ $0 <= 9_007_199_254_740_991 }) ?? true,
            response.attribution != .exact
                || (change.status != .untracked && change.insertions != nil)
        else {
            throw RunChangesPresentationError.invalidChange
        }
        rows.append(
            RunChangeRow(
                changeId: change.changeId,
                displayPath: change.displayPath,
                status: change.status,
                committed: change.committed,
                staged: change.staged,
                unstaged: change.unstaged,
                binary: change.binary,
                insertions: change.insertions,
                deletions: change.deletions,
                projectScope: change.projectScope
            )
        )
    }
    guard
        rows.isEmpty
            ? response.nextCursor == requestedAfterCursor && !response.hasMore
            : response.nextCursor == rows.last?.changeId
    else {
        throw RunChangesPresentationError.cursorMismatch
    }
    return ValidatedChangesPage(
        baselineHead: baselineHead,
        terminalHead: terminalHead,
        rows: rows
    )
}

private func validatedChangeHead(
    _ head: FlitRunChangeHead
) throws -> RunChangeHeadPresentation {
    switch head {
    case let .available(oid):
        guard
            [40, 64].contains(oid.utf8.count),
            oid.utf8.allSatisfy({ byte in
                (48 ... 57).contains(byte) || (97 ... 102).contains(byte)
            })
        else {
            throw RunChangesPresentationError.metadataMismatch
        }
        return .available(oid)
    case .unavailable:
        return .unavailable
    }
}

private func validOpaqueChangeId(_ value: String) -> Bool {
    value.utf8.count == 32
        && value.utf8.allSatisfy { byte in
            (48 ... 57).contains(byte) || (97 ... 102).contains(byte)
        }
}

private extension Optional {
    var isSome: Bool { self != nil }
}

struct RunChangesClient: Sendable {
    private let fixtureLoader:
        (@Sendable (FlitRunChangesReadRequest) throws -> FlitRunChangesReadResponse)?

    init(
        fixtureLoader:
            (@Sendable (FlitRunChangesReadRequest) throws -> FlitRunChangesReadResponse)? = nil
    ) {
        self.fixtureLoader = fixtureLoader
    }

    func loadPage(
        runId: String,
        expectedRunVersion: UInt64,
        afterCursor: String?
    ) throws -> FlitRunChangesReadResponse {
        let request = FlitRunChangesReadRequest(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            afterCursor: afterCursor,
            requestedChangeLimit: 50,
            clientProtocolVersion: flitClientProtocolVersion
        )
        if let fixtureLoader {
            return try fixtureLoader(request)
        }
        let requestData = try JSONEncoder().encode(request)
        let rendered = try runChangesReadJson(
            requestJson: String(decoding: requestData, as: UTF8.self)
        )
        return try JSONDecoder().decode(
            FlitRunChangesReadResponse.self,
            from: Data(rendered.utf8)
        )
    }

    func loadFirstPage(
        runId: String,
        expectedRunVersion: UInt64
    ) throws -> FlitRunChangesReadResponse {
        try loadPage(
            runId: runId,
            expectedRunVersion: expectedRunVersion,
            afterCursor: nil
        )
    }
}
