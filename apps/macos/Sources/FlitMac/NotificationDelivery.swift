import Foundation
import UserNotifications

enum StuckNotificationClientError: Error, Equatable {
    case contractMismatch
    case identityMismatch
    case invalidResponse
}

@MainActor
struct StuckNotificationClient {
    static let maximumDueNotifications = 100

    private let dueLoader:
        ((FlitManagedStuckNotificationsDueReadRequest) throws
            -> FlitManagedStuckNotificationsDueReadResponse)?
    private let claimLoader:
        ((FlitManagedStuckNotificationDeliveryClaimRequest) throws
            -> FlitManagedStuckNotificationDeliveryClaimResponse)?
    private let failureLoader:
        ((FlitManagedStuckNotificationDeliveryFailedRequest) throws
            -> FlitManagedStuckNotificationDeliveryFailedResponse)?
    private let receiptLoader:
        ((FlitManagedStuckNotificationDeliveredRequest) throws
            -> FlitManagedStuckNotificationDeliveredResponse)?

    init(
        dueLoader: ((FlitManagedStuckNotificationsDueReadRequest) throws
            -> FlitManagedStuckNotificationsDueReadResponse)? = nil,
        claimLoader: ((FlitManagedStuckNotificationDeliveryClaimRequest) throws
            -> FlitManagedStuckNotificationDeliveryClaimResponse)? = nil,
        failureLoader: ((FlitManagedStuckNotificationDeliveryFailedRequest) throws
            -> FlitManagedStuckNotificationDeliveryFailedResponse)? = nil,
        receiptLoader: ((FlitManagedStuckNotificationDeliveredRequest) throws
            -> FlitManagedStuckNotificationDeliveredResponse)? = nil
    ) {
        self.dueLoader = dueLoader
        self.claimLoader = claimLoader
        self.failureLoader = failureLoader
        self.receiptLoader = receiptLoader
    }

    func loadDue() throws -> [FlitManagedStuckNotificationDueRecord] {
        let request = FlitManagedStuckNotificationsDueReadRequest(
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitManagedStuckNotificationsDueReadResponse
        if let dueLoader {
            response = try dueLoader(request)
        } else {
            let requestData = try JSONEncoder().encode(request)
            let rendered = try managedStuckNotificationsDueReadJson(
                requestJson: String(decoding: requestData, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitManagedStuckNotificationsDueReadResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard
            response.protocolVersion == flitClientProtocolVersion,
            response.eventSchemaVersion == flitEventSchemaVersion,
            response.notifications.count <= Self.maximumDueNotifications
        else {
            throw StuckNotificationClientError.contractMismatch
        }
        var runIds = Set<String>()
        var occurrenceIds = Set<String>()
        for notification in response.notifications {
            guard
                boundedNotificationToken(notification.runId),
                notification.runVersion > 0,
                boundedNotificationToken(notification.occurrenceId),
                notification.platformId == notification.occurrenceId,
                runIds.insert(notification.runId).inserted,
                occurrenceIds.insert(notification.occurrenceId).inserted
            else {
                throw StuckNotificationClientError.invalidResponse
            }
        }
        return response.notifications
    }

    func claimDelivery(
        _ notification: FlitManagedStuckNotificationDueRecord
    ) throws -> FlitManagedStuckNotificationDeliveryClaimResponse {
        let request = FlitManagedStuckNotificationDeliveryClaimRequest(
            runId: notification.runId,
            expectedRunVersion: notification.runVersion,
            occurrenceId: notification.occurrenceId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitManagedStuckNotificationDeliveryClaimResponse
        if let claimLoader {
            response = try claimLoader(request)
        } else {
            let data = try JSONEncoder().encode(request)
            let rendered = try managedStuckNotificationDeliveryClaimJson(
                requestJson: String(decoding: data, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitManagedStuckNotificationDeliveryClaimResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard
            response.protocolVersion == flitClientProtocolVersion,
            response.runId == notification.runId,
            response.runVersion == notification.runVersion,
            response.occurrenceId == notification.occurrenceId,
            response.platformId == notification.platformId
        else {
            throw StuckNotificationClientError.identityMismatch
        }
        return response
    }

    func recordDeliveryFailure(
        _ notification: FlitManagedStuckNotificationDueRecord
    ) throws -> FlitManagedStuckNotificationDeliveryFailedResponse {
        let request = FlitManagedStuckNotificationDeliveryFailedRequest(
            runId: notification.runId,
            expectedRunVersion: notification.runVersion,
            occurrenceId: notification.occurrenceId,
            platformId: notification.platformId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitManagedStuckNotificationDeliveryFailedResponse
        if let failureLoader {
            response = try failureLoader(request)
        } else {
            let data = try JSONEncoder().encode(request)
            let rendered = try managedStuckNotificationDeliveryFailedJson(
                requestJson: String(decoding: data, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitManagedStuckNotificationDeliveryFailedResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard
            response.protocolVersion == flitClientProtocolVersion,
            response.runId == notification.runId,
            response.runVersion == notification.runVersion,
            response.occurrenceId == notification.occurrenceId,
            response.platformId == notification.platformId
        else {
            throw StuckNotificationClientError.identityMismatch
        }
        return response
    }

    @discardableResult
    func recordDelivered(
        _ notification: FlitManagedStuckNotificationDueRecord,
        platformId: String
    ) throws -> FlitManagedStuckNotificationDeliveredResponse {
        guard boundedNotificationToken(platformId) else {
            throw StuckNotificationClientError.invalidResponse
        }
        let request = FlitManagedStuckNotificationDeliveredRequest(
            runId: notification.runId,
            expectedRunVersion: notification.runVersion,
            occurrenceId: notification.occurrenceId,
            platformId: platformId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitManagedStuckNotificationDeliveredResponse
        if let receiptLoader {
            response = try receiptLoader(request)
        } else {
            let requestData = try JSONEncoder().encode(request)
            let rendered = try managedStuckNotificationDeliveredJson(
                requestJson: String(decoding: requestData, as: UTF8.self)
            )
            response = try JSONDecoder().decode(
                FlitManagedStuckNotificationDeliveredResponse.self,
                from: Data(rendered.utf8)
            )
        }
        guard
            response.protocolVersion == flitClientProtocolVersion,
            response.runId == notification.runId,
            response.occurrenceId == notification.occurrenceId,
            response.platformId == platformId
        else {
            throw StuckNotificationClientError.identityMismatch
        }
        switch response.status {
        case .delivered:
            guard
                response.previousVersion == notification.runVersion,
                response.eventId?.isEmpty == false,
                response.eventVersion.map({ $0 > notification.runVersion }) == true
            else {
                throw StuckNotificationClientError.invalidResponse
            }
        case .rejected:
            guard response.expectedRunVersion == notification.runVersion else {
                throw StuckNotificationClientError.invalidResponse
            }
        }
        return response
    }
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
        else { throw StuckNotificationClientError.contractMismatch }
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
            else { throw StuckNotificationClientError.invalidResponse }
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
            throw StuckNotificationClientError.identityMismatch
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
        else { throw StuckNotificationClientError.identityMismatch }
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
        content.categoryIdentifier = "dev.flit.possibly-stuck"
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        center.add(request) { error in
            Task { @MainActor in completion(error == nil) }
        }
    }
}

@MainActor
final class StuckNotificationCoordinator {
    private let client: StuckNotificationClient
    private let notificationClient: NotificationDeliveryClient?
    private let platform: any StuckNotificationPlatform
    private var active = true
    private var reconciliationInFlight = false
    private var scheduledIdentifiers = Set<String>()
    private var failedReleaseRetries:
        [String: FlitManagedStuckNotificationDueRecord] = [:]
    private var notificationFailedReleaseRetries:
        [String: FlitNotificationDeliveryRecord] = [:]

    init(
        client: StuckNotificationClient = StuckNotificationClient(),
        platform: any StuckNotificationPlatform
    ) {
        self.client = client
        notificationClient = nil
        self.platform = platform
    }

    init(
        notificationClient: NotificationDeliveryClient,
        platform: any StuckNotificationPlatform
    ) {
        client = StuckNotificationClient()
        self.notificationClient = notificationClient
        self.platform = platform
    }

    func reconcile(projectNamesByRunId: [String: String]) throws {
        guard active, !reconciliationInFlight else { return }
        if let notificationClient {
            try reconcileNotifications(
                client: notificationClient,
                projectNamesByRunId: projectNamesByRunId
            )
            return
        }
        retryFailedReleases()
        let due = try client.loadDue()
        guard !due.isEmpty else { return }
        let named = due.compactMap { notification -> NamedDueNotification? in
            guard
                let projectName = projectNamesByRunId[notification.runId],
                !projectName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                projectName.utf8.count <= 256
            else {
                return nil
            }
            return NamedDueNotification(notification: notification, projectName: projectName)
        }
        guard named.count == due.count else {
            throw StuckNotificationClientError.identityMismatch
        }
        reconciliationInFlight = true
        platform.authorizationState { [weak self] state in
            guard let self, self.active else { return }
            switch state {
            case .notDetermined:
                self.platform.requestAuthorization { [weak self] granted in
                    guard let self, self.active else { return }
                    if granted {
                        self.reconcileDelivered(named)
                    } else {
                        self.finishReconciliation()
                    }
                }
            case .denied:
                self.finishReconciliation()
            case .authorized:
                self.reconcileDelivered(named)
            }
        }
    }

    func stop() {
        active = false
        reconciliationInFlight = false
        scheduledIdentifiers.removeAll()
        failedReleaseRetries.removeAll()
        notificationFailedReleaseRetries.removeAll()
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
            else { throw StuckNotificationClientError.identityMismatch }
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
                    self.notificationFailedReleaseRetries.removeValue(forKey: identifier)
                } catch {
                    self.notificationFailedReleaseRetries[identifier] = next.notification
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
        for (identifier, item) in Array(notificationFailedReleaseRetries) {
            do {
                _ = try client.failed(item)
                notificationFailedReleaseRetries.removeValue(forKey: identifier)
            } catch { continue }
        }
    }

    private func reconcileDelivered(_ due: [NamedDueNotification]) {
        platform.deliveredIdentifiers { [weak self] delivered in
            guard let self, self.active else { return }
            self.recordDelivered(due, identifiers: delivered)
            let pending = due.filter {
                !$0.notification.deliveryClaimed
                    && !delivered.contains($0.notification.platformId)
                    && !self.scheduledIdentifiers.contains($0.notification.occurrenceId)
            }
            guard !pending.isEmpty else {
                self.finishReconciliation()
                return
            }
            self.schedule(pending, allDue: due)
        }
    }

    private func schedule(
        _ pending: [NamedDueNotification],
        allDue: [NamedDueNotification]
    ) {
        guard let next = pending.first else {
            verifyScheduledDelivery(allDue)
            return
        }
        let identifier = next.notification.occurrenceId
        do {
            _ = try client.claimDelivery(next.notification)
        } catch {
            schedule(Array(pending.dropFirst()), allDue: allDue)
            return
        }
        scheduledIdentifiers.insert(identifier)
        platform.add(
            identifier: identifier,
            title: FoundationCopy.text(.notificationStuckTitle),
            body: FoundationCopy.format(.notificationStuckBody, next.projectName)
        ) { [weak self] accepted in
            guard let self, self.active else { return }
            if !accepted {
                self.scheduledIdentifiers.remove(identifier)
                do {
                    _ = try self.client.recordDeliveryFailure(next.notification)
                    self.failedReleaseRetries.removeValue(forKey: identifier)
                } catch {
                    self.failedReleaseRetries[identifier] = next.notification
                }
            }
            self.schedule(Array(pending.dropFirst()), allDue: allDue)
        }
    }

    private func verifyScheduledDelivery(_ due: [NamedDueNotification]) {
        platform.deliveredIdentifiers { [weak self] delivered in
            guard let self, self.active else { return }
            self.recordDelivered(due, identifiers: delivered)
            self.finishReconciliation()
        }
    }

    private func recordDelivered(
        _ due: [NamedDueNotification],
        identifiers: Set<String>
    ) {
        for item in due where identifiers.contains(item.notification.platformId) {
            do {
                let response = try client.recordDelivered(
                    item.notification,
                    platformId: item.notification.platformId
                )
                if response.status == .delivered {
                    scheduledIdentifiers.remove(item.notification.occurrenceId)
                }
            } catch {
                continue
            }
        }
    }

    private func finishReconciliation() {
        reconciliationInFlight = false
    }

    private func retryFailedReleases() {
        for (identifier, notification) in Array(failedReleaseRetries) {
            do {
                _ = try client.recordDeliveryFailure(notification)
                failedReleaseRetries.removeValue(forKey: identifier)
            } catch {
                continue
            }
        }
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

private struct NamedDueNotification {
    let notification: FlitManagedStuckNotificationDueRecord
    let projectName: String
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
