import Foundation
import UserNotifications

enum NotificationDeliveryClientError: Error, Equatable {
    case contractMismatch
    case identityMismatch
    case invalidResponse
}

@MainActor
struct NotificationDeliveryClient {
    static let maximumDueNotifications = 1_000

    private let dueLoader: ((FlitNotificationDeliveriesDueReadRequest) throws
        -> FlitNotificationDeliveriesDueReadResponse)?
    private let claimLoader: ((FlitNotificationDeliveryClaimRequest) throws
        -> FlitNotificationDeliveryClaimResponse)?
    private let failureLoader: ((FlitNotificationDeliveryFailedRequest) throws
        -> FlitNotificationDeliveryFailedResponse)?
    private let receiptLoader: ((FlitNotificationDeliveredRequest) throws
        -> FlitNotificationDeliveredResponse)?

    init(
        dueLoader: ((FlitNotificationDeliveriesDueReadRequest) throws
            -> FlitNotificationDeliveriesDueReadResponse)? = nil,
        claimLoader: ((FlitNotificationDeliveryClaimRequest) throws
            -> FlitNotificationDeliveryClaimResponse)? = nil,
        failureLoader: ((FlitNotificationDeliveryFailedRequest) throws
            -> FlitNotificationDeliveryFailedResponse)? = nil,
        receiptLoader: ((FlitNotificationDeliveredRequest) throws
            -> FlitNotificationDeliveredResponse)? = nil
    ) {
        self.dueLoader = dueLoader
        self.claimLoader = claimLoader
        self.failureLoader = failureLoader
        self.receiptLoader = receiptLoader
    }

    func loadDue(localMinute: UInt16) throws -> [FlitNotificationDeliveryRecord] {
        let request = FlitNotificationDeliveriesDueReadRequest(
            localMinute: localMinute,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitNotificationDeliveriesDueReadResponse
        if let dueLoader { response = try dueLoader(request) }
        else {
            let data = try JSONEncoder().encode(request)
            let rendered = try notificationDeliveriesDueReadJson(
                requestJson: String(decoding: data, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitNotificationDeliveriesDueReadResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard
            response.protocolVersion == flitClientProtocolVersion,
            response.notifications.count <= Self.maximumDueNotifications
        else { throw NotificationDeliveryClientError.contractMismatch }
        var notificationIds = Set<String>()
        var platformIds = Set<String>()
        for item in response.notifications {
            guard
                boundedNotificationToken(item.notificationId, maximumBytes: 96),
                boundedNotificationToken(item.runId), item.runVersion > 0,
                boundedNotificationToken(item.projectId),
                boundedNotificationToken(item.itemId), item.itemVersion > 0,
                boundedNotificationToken(item.platformId),
                notificationIds.insert(item.notificationId).inserted,
                platformIds.insert(item.platformId).inserted,
                !item.deliveryClaimed || !item.catchUp
            else { throw NotificationDeliveryClientError.invalidResponse }
        }
        return response.notifications
    }

    func claim(
        _ item: FlitNotificationDeliveryRecord,
        localMinute: UInt16
    ) throws -> FlitNotificationDeliveryClaimResponse {
        let request = FlitNotificationDeliveryClaimRequest(
            notificationId: item.notificationId, runId: item.runId,
            expectedRunVersion: item.runVersion, kind: item.kind,
            itemId: item.itemId, itemVersion: item.itemVersion,
            platformId: item.platformId, localMinute: localMinute,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response = try claimLoader?(request) ?? exactResponse(
            request, call: notificationDeliveryClaimJson,
            as: FlitNotificationDeliveryClaimResponse.self, item: item
        )
        try validate(response, item: item)
        guard response.runVersion == item.runVersion else {
            throw NotificationDeliveryClientError.identityMismatch
        }
        return response
    }

    func failed(
        _ item: FlitNotificationDeliveryRecord
    ) throws -> FlitNotificationDeliveryFailedResponse {
        let request = FlitNotificationDeliveryFailedRequest(
            notificationId: item.notificationId, runId: item.runId, kind: item.kind,
            itemId: item.itemId, itemVersion: item.itemVersion,
            platformId: item.platformId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response = try failureLoader?(request) ?? exactResponse(
            request, call: notificationDeliveryFailedJson,
            as: FlitNotificationDeliveryFailedResponse.self, item: item
        )
        try validate(response, item: item)
        return response
    }

    func delivered(
        _ item: FlitNotificationDeliveryRecord
    ) throws -> FlitNotificationDeliveredResponse {
        let request = FlitNotificationDeliveredRequest(
            notificationId: item.notificationId, runId: item.runId, kind: item.kind,
            itemId: item.itemId, itemVersion: item.itemVersion,
            platformId: item.platformId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response = try receiptLoader?(request) ?? exactResponse(
            request, call: notificationDeliveredJson,
            as: FlitNotificationDeliveredResponse.self, item: item
        )
        try validate(response, item: item)
        return response
    }

    private func exactResponse<Request: Encodable, Response: Codable>(
        _ request: Request,
        call: (String) throws -> String,
        as _: Response.Type,
        item: FlitNotificationDeliveryRecord
    ) throws -> Response {
        let data = try JSONEncoder().encode(request)
        let rendered = try call(String(decoding: data, as: UTF8.self))
        let response = try JSONDecoder().decode(Response.self, from: Data(rendered.utf8))
        try validate(response, item: item)
        return response
    }

    private func validate<Response: Codable>(
        _ response: Response,
        item: FlitNotificationDeliveryRecord
    ) throws {
        let encoded = try JSONEncoder().encode(response)
        let identity = try JSONDecoder().decode(
            NotificationDeliveryResponseIdentity.self,
            from: encoded
        )
        guard
            identity.protocolVersion == flitClientProtocolVersion,
            identity.notificationId == item.notificationId,
            identity.runId == item.runId, identity.kind == item.kind,
            identity.itemId == item.itemId, identity.itemVersion == item.itemVersion,
            identity.platformId == item.platformId
        else { throw NotificationDeliveryClientError.identityMismatch }
    }
}

private struct NotificationDeliveryResponseIdentity: Decodable {
    let protocolVersion: String
    let notificationId: String
    let runId: String
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case notificationId = "notification_id"
        case runId = "run_id"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
    }
}

private func boundedNotificationToken(_ value: String, maximumBytes: Int = 256) -> Bool {
    !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        && value.utf8.count <= maximumBytes
        && !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
}

func currentLocalNotificationMinute(calendar: Calendar = .current, date: Date = Date()) -> UInt16 {
    let components = calendar.dateComponents([.hour, .minute], from: date)
    let hour = components.hour ?? 0
    let minute = components.minute ?? 0
    return UInt16(hour * 60 + minute)
}

enum NotificationAuthorizationState: Equatable {
    case notDetermined
    case denied
    case authorized
}

enum StuckNotificationPresentationPolicy {
    static let foregroundOptions: UNNotificationPresentationOptions = [.banner]
}

final class StuckNotificationPresentationDelegate: NSObject,
    UNUserNotificationCenterDelegate,
    @unchecked Sendable
{
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler:
            @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler(StuckNotificationPresentationPolicy.foregroundOptions)
    }
}

@MainActor
protocol StuckNotificationPlatform: AnyObject {
    func authorizationState(
        completion: @escaping @MainActor @Sendable (NotificationAuthorizationState) -> Void
    )
    func requestAuthorization(
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    )
    func deliveredIdentifiers(
        completion: @escaping @MainActor @Sendable (Set<String>) -> Void
    )
    func add(
        identifier: String,
        title: String,
        body: String,
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    )
}

@MainActor
final class UserNotificationPlatform: StuckNotificationPlatform {
    private let center: UNUserNotificationCenter
    private let presentationDelegate: StuckNotificationPresentationDelegate

    init(center: UNUserNotificationCenter) {
        self.center = center
        presentationDelegate = StuckNotificationPresentationDelegate()
        center.delegate = presentationDelegate
    }

    func authorizationState(
        completion: @escaping @MainActor @Sendable (NotificationAuthorizationState) -> Void
    ) {
        center.getNotificationSettings { settings in
            let state: NotificationAuthorizationState = switch settings.authorizationStatus {
            case .notDetermined: .notDetermined
            case .denied: .denied
            case .authorized, .provisional, .ephemeral: .authorized
            @unknown default: .denied
            }
            Task { @MainActor in completion(state) }
        }
    }

    func requestAuthorization(
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    ) {
        center.requestAuthorization(options: [.alert]) { granted, _ in
            Task { @MainActor in completion(granted) }
        }
    }

    func deliveredIdentifiers(
        completion: @escaping @MainActor @Sendable (Set<String>) -> Void
    ) {
        center.getDeliveredNotifications { notifications in
            let identifiers = Set(notifications.map(\.request.identifier))
            Task { @MainActor in completion(identifiers) }
        }
    }

    func add(
        identifier: String,
        title: String,
        body: String,
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    ) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.categoryIdentifier = "dev.flit.attention"
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        center.add(request) { error in
            Task { @MainActor in completion(error == nil) }
        }
    }
}

@MainActor
final class StuckNotificationCoordinator {
    private let notificationClient: NotificationDeliveryClient
    private let platform: any StuckNotificationPlatform
    private var active = true
    private var reconciliationInFlight = false
    private var scheduledIdentifiers = Set<String>()
    private var failedReleaseRetries:
        [String: FlitNotificationDeliveryRecord] = [:]

    init(
        notificationClient: NotificationDeliveryClient,
        platform: any StuckNotificationPlatform
    ) {
        self.notificationClient = notificationClient
        self.platform = platform
    }

    func reconcile(projectNamesByRunId: [String: String]) throws {
        guard active, !reconciliationInFlight else { return }
        try reconcileNotifications(
            client: notificationClient,
            projectNamesByRunId: projectNamesByRunId
        )
    }

    func stop() {
        active = false
        reconciliationInFlight = false
        scheduledIdentifiers.removeAll()
        failedReleaseRetries.removeAll()
    }

    private func reconcileNotifications(
        client: NotificationDeliveryClient,
        projectNamesByRunId: [String: String]
    ) throws {
        retryNotificationFailedReleases(client: client)
        let localMinute = currentLocalNotificationMinute()
        let due = try client.loadDue(localMinute: localMinute)
        guard !due.isEmpty else { return }
        let named = try due.map { item -> NamedNotification in
            guard
                let projectName = projectNamesByRunId[item.runId],
                !projectName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                projectName.utf8.count <= 256
            else { throw NotificationDeliveryClientError.identityMismatch }
            return NamedNotification(notification: item, projectName: projectName)
        }
        reconciliationInFlight = true
        platform.authorizationState { [weak self] state in
            guard let self, self.active else { return }
            switch state {
            case .notDetermined:
                self.platform.requestAuthorization { [weak self] granted in
                    guard let self, self.active else { return }
                    if granted { self.reconcileNotificationDelivered(named, client: client) }
                    else { self.finishReconciliation() }
                }
            case .denied:
                self.finishReconciliation()
            case .authorized:
                self.reconcileNotificationDelivered(named, client: client)
            }
        }
    }

    private func reconcileNotificationDelivered(
        _ due: [NamedNotification],
        client: NotificationDeliveryClient
    ) {
        platform.deliveredIdentifiers { [weak self] delivered in
            guard let self, self.active else { return }
            self.recordNotificationDelivered(due, identifiers: delivered, client: client)
            let pending = due.filter {
                !$0.notification.deliveryClaimed
                    && !delivered.contains($0.notification.platformId)
                    && !self.scheduledIdentifiers.contains($0.notification.platformId)
            }
            guard !pending.isEmpty else {
                self.finishReconciliation()
                return
            }
            self.scheduleNotifications(pending, allDue: due, client: client)
        }
    }

    private func scheduleNotifications(
        _ pending: [NamedNotification],
        allDue: [NamedNotification],
        client: NotificationDeliveryClient
    ) {
        guard let next = pending.first else {
            platform.deliveredIdentifiers { [weak self] delivered in
                guard let self, self.active else { return }
                self.recordNotificationDelivered(allDue, identifiers: delivered, client: client)
                self.finishReconciliation()
            }
            return
        }
        let identifier = next.notification.platformId
        do {
            _ = try client.claim(
                next.notification,
                localMinute: currentLocalNotificationMinute()
            )
        } catch {
            scheduleNotifications(Array(pending.dropFirst()), allDue: allDue, client: client)
            return
        }
        scheduledIdentifiers.insert(identifier)
        platform.add(
            identifier: identifier,
            title: FoundationCopy.text(.notificationStuckTitle),
            body: notificationBody(next)
        ) { [weak self] accepted in
            guard let self, self.active else { return }
            if !accepted {
                self.scheduledIdentifiers.remove(identifier)
                do {
                    _ = try client.failed(next.notification)
                    self.failedReleaseRetries.removeValue(forKey: identifier)
                } catch {
                    self.failedReleaseRetries[identifier] = next.notification
                }
            }
            self.scheduleNotifications(
                Array(pending.dropFirst()), allDue: allDue, client: client
            )
        }
    }

    private func recordNotificationDelivered(
        _ due: [NamedNotification],
        identifiers: Set<String>,
        client: NotificationDeliveryClient
    ) {
        for item in due where identifiers.contains(item.notification.platformId) {
            do {
                _ = try client.delivered(item.notification)
                scheduledIdentifiers.remove(item.notification.platformId)
            } catch { continue }
        }
    }

    private func retryNotificationFailedReleases(client: NotificationDeliveryClient) {
        for (identifier, item) in Array(failedReleaseRetries) {
            do {
                _ = try client.failed(item)
                failedReleaseRetries.removeValue(forKey: identifier)
            } catch { continue }
        }
    }

    private func finishReconciliation() {
        reconciliationInFlight = false
    }
}

@MainActor
final class InactiveStuckNotificationPlatform: StuckNotificationPlatform {
    func authorizationState(
        completion: @escaping @MainActor @Sendable (NotificationAuthorizationState) -> Void
    ) {
        completion(.denied)
    }

    func requestAuthorization(
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    ) {
        completion(false)
    }

    func deliveredIdentifiers(
        completion: @escaping @MainActor @Sendable (Set<String>) -> Void
    ) {
        completion([])
    }

    func add(
        identifier: String,
        title: String,
        body: String,
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    ) {
        completion(false)
    }
}

@MainActor
enum StuckNotificationCoordinatorFactory {
    static func makeDefault() -> StuckNotificationCoordinator {
        #if FLIT_NATIVE_TESTS
            StuckNotificationCoordinator(
                notificationClient: NotificationDeliveryClient(),
                platform: InactiveStuckNotificationPlatform()
            )
        #else
            StuckNotificationCoordinator(
                notificationClient: NotificationDeliveryClient(),
                platform: UserNotificationPlatform(center: .current())
            )
        #endif
    }
}

private struct NamedNotification {
    let notification: FlitNotificationDeliveryRecord
    let projectName: String
}

private func notificationBody(_ item: NamedNotification) -> String {
    let key: FoundationCopyKey = switch item.notification.kind {
    case .permission: .notificationPermissionBody
    case .question: .notificationQuestionBody
    case .failure: .notificationFailureBody
    case .completion: .notificationCompletionBody
    case .stuck: .notificationStuckBody
    }
    return FoundationCopy.format(key, item.projectName)
}
