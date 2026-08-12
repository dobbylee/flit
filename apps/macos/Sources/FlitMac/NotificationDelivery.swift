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

private func boundedNotificationToken(_ value: String) -> Bool {
    !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        && value.utf8.count <= 256
        && !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
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
    private let platform: any StuckNotificationPlatform
    private var active = true
    private var reconciliationInFlight = false
    private var scheduledIdentifiers = Set<String>()
    private var failedReleaseRetries:
        [String: FlitManagedStuckNotificationDueRecord] = [:]

    init(
        client: StuckNotificationClient = StuckNotificationClient(),
        platform: any StuckNotificationPlatform
    ) {
        self.client = client
        self.platform = platform
    }

    func reconcile(projectNamesByRunId: [String: String]) throws {
        guard active, !reconciliationInFlight else { return }
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
            StuckNotificationCoordinator(platform: InactiveStuckNotificationPlatform())
        #else
            StuckNotificationCoordinator(
                platform: UserNotificationPlatform(center: .current())
            )
        #endif
    }
}

private struct NamedDueNotification {
    let notification: FlitManagedStuckNotificationDueRecord
    let projectName: String
}
