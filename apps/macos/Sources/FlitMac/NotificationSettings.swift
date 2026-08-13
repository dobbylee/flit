import AppKit
import Foundation
import SwiftUI

enum NotificationSettingsClientError: Error, Equatable {
    case command(FlitCommandErrorCode)
    case invalidResponse
    case pageLimitExceeded
}

struct NotificationKindsDraft: Equatable {
    var permission: Bool
    var question: Bool
    var failure: Bool
    var completion: Bool
    var stuck: Bool

    init(_ kinds: FlitNotificationKinds) {
        permission = kinds.permission
        question = kinds.question
        failure = kinds.failure
        completion = kinds.completion
        stuck = kinds.stuck
    }

    var protocolValue: FlitNotificationKinds {
        FlitNotificationKinds(
            permission: permission,
            question: question,
            failure: failure,
            completion: completion,
            stuck: stuck
        )
    }
}

struct NotificationOverrideDraft: Equatable {
    var permission: FlitNotificationOverride
    var question: FlitNotificationOverride
    var failure: FlitNotificationOverride
    var completion: FlitNotificationOverride
    var stuck: FlitNotificationOverride

    init(_ kinds: FlitNotificationKindOverrides) {
        permission = kinds.permission
        question = kinds.question
        failure = kinds.failure
        completion = kinds.completion
        stuck = kinds.stuck
    }

    var protocolValue: FlitNotificationKindOverrides {
        FlitNotificationKindOverrides(
            permission: permission,
            question: question,
            failure: failure,
            completion: completion,
            stuck: stuck
        )
    }
}

func notificationWallTimeMinute(_ rendered: String) -> UInt16? {
    let characters = Array(rendered)
    guard
        characters.count == 5,
        characters[2] == ":",
        [0, 1, 3, 4].allSatisfy({ index in
            characters[index].asciiValue.map { (48 ... 57).contains($0) } == true
        }),
        let hour = Int(String(characters[0 ... 1])),
        let minute = Int(String(characters[3 ... 4])),
        (0 ... 23).contains(hour),
        (0 ... 59).contains(minute)
    else {
        return nil
    }
    return UInt16(hour * 60 + minute)
}

func notificationWallTimeText(_ minute: UInt16) -> String? {
    guard minute < 1_440 else { return nil }
    return String(format: "%02d:%02d", minute / 60, minute % 60)
}

@MainActor
struct NotificationPolicyClient {
    private let readLoader:
        ((FlitNotificationPolicyReadRequest) throws -> FlitNotificationPolicyResponse)?
    private let globalUpdateLoader:
        ((FlitGlobalNotificationPolicyUpdateRequest) throws
            -> FlitNotificationPolicyResponse)?
    private let projectUpdateLoader:
        ((FlitProjectNotificationPolicyUpdateRequest) throws
            -> FlitNotificationPolicyResponse)?
    private let now: () -> String

    init(
        readLoader:
            ((FlitNotificationPolicyReadRequest) throws
                -> FlitNotificationPolicyResponse)? = nil,
        globalUpdateLoader:
            ((FlitGlobalNotificationPolicyUpdateRequest) throws
                -> FlitNotificationPolicyResponse)? = nil,
        projectUpdateLoader:
            ((FlitProjectNotificationPolicyUpdateRequest) throws
                -> FlitNotificationPolicyResponse)? = nil,
        now: @escaping () -> String = {
            ISO8601DateFormatter().string(from: Date())
        }
    ) {
        self.readLoader = readLoader
        self.globalUpdateLoader = globalUpdateLoader
        self.projectUpdateLoader = projectUpdateLoader
        self.now = now
    }

    func read(projectId: String?) throws -> FlitNotificationPolicyResponse {
        let request = FlitNotificationPolicyReadRequest(
            projectId: projectId,
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitNotificationPolicyResponse
        if let readLoader {
            response = try readLoader(request)
        } else {
            response = try decode(
                notificationPolicyReadJson(requestJson: try encode(request))
            )
        }
        try validate(response, expectedProjectId: projectId)
        return response
    }

    func updateGlobal(
        expectedVersion: UInt64,
        kinds: FlitNotificationKinds,
        quietHours: FlitQuietHours
    ) throws -> FlitNotificationPolicyResponse {
        let request = FlitGlobalNotificationPolicyUpdateRequest(
            expectedVersion: expectedVersion,
            kinds: kinds,
            quietHours: quietHours,
            updatedAt: now(),
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitNotificationPolicyResponse
        if let globalUpdateLoader {
            response = try globalUpdateLoader(request)
        } else {
            response = try decode(
                notificationPolicyUpdateGlobalJson(requestJson: try encode(request))
            )
        }
        try validate(response, expectedProjectId: nil)
        guard response.global.version == expectedVersion + 1 else {
            throw NotificationSettingsClientError.invalidResponse
        }
        return response
    }

    func updateProject(
        projectId: String,
        expectedVersion: UInt64,
        master: FlitProjectNotificationMaster,
        kinds: FlitNotificationKindOverrides
    ) throws -> FlitNotificationPolicyResponse {
        let request = FlitProjectNotificationPolicyUpdateRequest(
            projectId: projectId,
            expectedVersion: expectedVersion,
            master: master,
            kinds: kinds,
            updatedAt: now(),
            clientProtocolVersion: flitClientProtocolVersion
        )
        let response: FlitNotificationPolicyResponse
        if let projectUpdateLoader {
            response = try projectUpdateLoader(request)
        } else {
            response = try decode(
                notificationPolicyUpdateProjectJson(requestJson: try encode(request))
            )
        }
        try validate(response, expectedProjectId: projectId)
        guard response.project?.version == expectedVersion + 1 else {
            throw NotificationSettingsClientError.invalidResponse
        }
        return response
    }

    private func encode<T: Encodable>(_ value: T) throws -> String {
        String(decoding: try JSONEncoder().encode(value), as: UTF8.self)
    }

    private func decode(_ rendered: String) throws -> FlitNotificationPolicyResponse {
        let data = Data(rendered.utf8)
        if let error = try? JSONDecoder().decode(FlitCommandError.self, from: data) {
            throw NotificationSettingsClientError.command(error.code)
        }
        return try JSONDecoder().decode(FlitNotificationPolicyResponse.self, from: data)
    }

    private func validate(
        _ response: FlitNotificationPolicyResponse,
        expectedProjectId: String?
    ) throws {
        guard
            response.protocolVersion == flitClientProtocolVersion,
            response.effective.globalVersion == response.global.version,
            response.effective.projectVersion == response.project?.version,
            response.global.quietHours.startMinute < 1_440,
            response.global.quietHours.endMinute < 1_440,
            !response.global.quietHours.enabled
                || response.global.quietHours.startMinute != response.global.quietHours.endMinute,
            (expectedProjectId == nil) == (response.project == nil)
        else {
            throw NotificationSettingsClientError.invalidResponse
        }
    }
}

@MainActor
struct NotificationSettingsProjectClient {
    static let pageLimit: UInt32 = 50
    static let maximumPages = 20

    private let pageLoader:
        ((FlitProjectListCursor?) throws -> FlitProjectsListResponse)?

    init(
        pageLoader: ((FlitProjectListCursor?) throws -> FlitProjectsListResponse)? = nil
    ) {
        self.pageLoader = pageLoader
    }

    func loadAll() throws -> [FlitProjectRecord] {
        var cursor: FlitProjectListCursor?
        var projects: [FlitProjectRecord] = []
        var identities = Set<String>()
        for _ in 0..<Self.maximumPages {
            let response: FlitProjectsListResponse
            if let pageLoader {
                response = try pageLoader(cursor)
            } else {
                let rendered = try projectsListPageJson(
                    afterDisplayName: cursor?.displayName,
                    afterProjectId: cursor?.projectId,
                    limit: Self.pageLimit,
                    clientProtocolVersion: flitClientProtocolVersion
                )
                let data = Data(rendered.utf8)
                if let error = try? JSONDecoder().decode(FlitCommandError.self, from: data) {
                    throw NotificationSettingsClientError.command(error.code)
                }
                response = try JSONDecoder().decode(
                    FlitProjectsListResponse.self,
                    from: data
                )
            }
            guard
                response.protocolVersion == flitClientProtocolVersion,
                response.projects.count <= Int(Self.pageLimit),
                response.projects.allSatisfy({
                    !$0.id.isEmpty && !$0.displayName.isEmpty && identities.insert($0.id).inserted
                })
            else {
                throw NotificationSettingsClientError.invalidResponse
            }
            projects.append(contentsOf: response.projects)
            guard let next = response.nextCursor else { return projects }
            guard
                response.projects.count == Int(Self.pageLimit),
                next != cursor,
                response.projects.last.map({
                    $0.id == next.projectId && $0.displayName == next.displayName
                }) == true
            else {
                throw NotificationSettingsClientError.invalidResponse
            }
            cursor = next
        }
        throw NotificationSettingsClientError.pageLimitExceeded
    }
}

@MainActor
final class NotificationSettingsModel: ObservableObject {
    @Published private(set) var projects: [FlitProjectRecord] = []
    @Published private(set) var selectedProjectId: String?
    @Published private(set) var effectiveKinds: FlitNotificationKinds?
    @Published private(set) var isLoaded = false
    @Published private(set) var isBusy = false
    @Published private(set) var errorCopy: String?
    @Published var globalKinds = NotificationKindsDraft(
        FlitNotificationKinds(
            permission: true,
            question: true,
            failure: true,
            completion: false,
            stuck: true
        )
    )
    @Published var quietHoursEnabled = false
    @Published var quietHoursStart = "22:00"
    @Published var quietHoursEnd = "08:00"
    @Published var projectMaster = FlitProjectNotificationMaster.inherit
    @Published var projectKinds = NotificationOverrideDraft(
        FlitNotificationKindOverrides(
            permission: .inherit,
            question: .inherit,
            failure: .inherit,
            completion: .inherit,
            stuck: .inherit
        )
    )

    private let policyClient: NotificationPolicyClient
    private let projectClient: NotificationSettingsProjectClient
    private var globalVersion: UInt64?
    private var projectVersion: UInt64?

    init(
        policyClient: NotificationPolicyClient = NotificationPolicyClient(),
        projectClient: NotificationSettingsProjectClient =
            NotificationSettingsProjectClient()
    ) {
        self.policyClient = policyClient
        self.projectClient = projectClient
    }

    func loadIfNeeded() {
        guard !isLoaded, !isBusy else { return }
        reload()
    }

    func reload() {
        guard !isBusy else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            let loadedProjects = try projectClient.loadAll()
            let retainedSelection = selectedProjectId.flatMap { selected in
                loadedProjects.contains(where: { $0.id == selected }) ? selected : nil
            }
            let candidateSelection = retainedSelection ?? loadedProjects.first?.id
            let response = try policyClient.read(projectId: candidateSelection)
            try apply(response)
            projects = loadedProjects
            selectedProjectId = candidateSelection
            errorCopy = nil
            isLoaded = true
        } catch {
            errorCopy = FoundationCopy.text(.notificationSettingsUnavailable)
        }
    }

    func selectProject(_ projectId: String?) {
        guard projectId != selectedProjectId, !isBusy else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            let globalDraft = currentGlobalDraft
            let response = try policyClient.read(projectId: projectId)
            selectedProjectId = projectId
            try apply(response)
            restoreGlobalDraft(globalDraft)
            errorCopy = nil
        } catch {
            errorCopy = FoundationCopy.text(.notificationSettingsUnavailable)
        }
    }

    func saveGlobal() {
        guard let globalVersion, !isBusy else { return }
        guard
            let start = notificationWallTimeMinute(quietHoursStart),
            let end = notificationWallTimeMinute(quietHoursEnd),
            !quietHoursEnabled || start != end
        else {
            errorCopy = FoundationCopy.text(.notificationSettingsInvalidTime)
            return
        }
        isBusy = true
        defer { isBusy = false }
        do {
            let projectDraft = currentProjectDraft
            let updated = try policyClient.updateGlobal(
                expectedVersion: globalVersion,
                kinds: globalKinds.protocolValue,
                quietHours: FlitQuietHours(
                    enabled: quietHoursEnabled,
                    startMinute: start,
                    endMinute: end
                )
            )
            do {
                try apply(policyClient.read(projectId: selectedProjectId))
                restoreProjectDraft(projectDraft)
                errorCopy = nil
            } catch {
                try applyGlobal(updated.global)
                errorCopy = FoundationCopy.text(.notificationSettingsUnavailable)
            }
        } catch {
            errorCopy = FoundationCopy.text(.notificationSettingsSaveFailed)
        }
    }

    func saveProject() {
        guard
            let selectedProjectId,
            let projectVersion,
            !isBusy
        else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            let globalDraft = currentGlobalDraft
            let response = try policyClient.updateProject(
                projectId: selectedProjectId,
                expectedVersion: projectVersion,
                master: projectMaster,
                kinds: projectKinds.protocolValue
            )
            try apply(response)
            restoreGlobalDraft(globalDraft)
            errorCopy = nil
        } catch {
            errorCopy = FoundationCopy.text(.notificationSettingsSaveFailed)
        }
    }

    private func applyGlobal(_ global: FlitGlobalNotificationPolicy) throws {
        guard
            let start = notificationWallTimeText(global.quietHours.startMinute),
            let end = notificationWallTimeText(global.quietHours.endMinute)
        else {
            throw NotificationSettingsClientError.invalidResponse
        }
        globalVersion = global.version
        globalKinds = NotificationKindsDraft(global.kinds)
        quietHoursEnabled = global.quietHours.enabled
        quietHoursStart = start
        quietHoursEnd = end
    }

    private var currentGlobalDraft: (
        kinds: NotificationKindsDraft,
        enabled: Bool,
        start: String,
        end: String,
        version: UInt64?
    ) {
        (
            globalKinds,
            quietHoursEnabled,
            quietHoursStart,
            quietHoursEnd,
            globalVersion
        )
    }

    private func restoreGlobalDraft(
        _ draft: (
            kinds: NotificationKindsDraft,
            enabled: Bool,
            start: String,
            end: String,
            version: UInt64?
        )
    ) {
        globalKinds = draft.kinds
        quietHoursEnabled = draft.enabled
        quietHoursStart = draft.start
        quietHoursEnd = draft.end
        globalVersion = draft.version
    }

    private var currentProjectDraft: (
        master: FlitProjectNotificationMaster,
        kinds: NotificationOverrideDraft,
        version: UInt64?
    ) {
        (projectMaster, projectKinds, projectVersion)
    }

    private func restoreProjectDraft(
        _ draft: (
            master: FlitProjectNotificationMaster,
            kinds: NotificationOverrideDraft,
            version: UInt64?
        )
    ) {
        projectMaster = draft.master
        projectKinds = draft.kinds
        projectVersion = draft.version
    }

    private func apply(_ response: FlitNotificationPolicyResponse) throws {
        try applyGlobal(response.global)
        projectVersion = response.project?.version
        if let project = response.project {
            projectMaster = project.master
            projectKinds = NotificationOverrideDraft(project.kinds)
        } else {
            projectMaster = .inherit
            projectKinds = NotificationOverrideDraft(
                FlitNotificationKindOverrides(
                    permission: .inherit,
                    question: .inherit,
                    failure: .inherit,
                    completion: .inherit,
                    stuck: .inherit
                )
            )
        }
        effectiveKinds = response.effective.kinds
    }

    #if FLIT_NATIVE_TESTS
        var globalVersionForTesting: UInt64? { globalVersion }
        var projectVersionForTesting: UInt64? { projectVersion }
    #endif
}

private struct NotificationSettingsView: View {
    @ObservedObject var model: NotificationSettingsModel

    var body: some View {
        Form {
            if let errorCopy = model.errorCopy {
                Section {
                    HStack {
                        Label(errorCopy, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                        Spacer()
                        Button(FoundationCopy.text(.notificationSettingsReload)) {
                            model.reload()
                        }
                        .accessibilityIdentifier("flit.settings.notifications.reload")
                    }
                }
            }

            Section(FoundationCopy.text(.notificationSettingsGlobal)) {
                Toggle(
                    FoundationCopy.text(.notificationSettingsPermission),
                    isOn: $model.globalKinds.permission
                )
                Toggle(
                    FoundationCopy.text(.notificationSettingsQuestion),
                    isOn: $model.globalKinds.question
                )
                Toggle(
                    FoundationCopy.text(.notificationSettingsFailure),
                    isOn: $model.globalKinds.failure
                )
                Toggle(
                    FoundationCopy.text(.notificationSettingsCompletion),
                    isOn: $model.globalKinds.completion
                )
                Toggle(
                    FoundationCopy.text(.notificationSettingsStuck),
                    isOn: $model.globalKinds.stuck
                )
                Toggle(
                    FoundationCopy.text(.notificationSettingsQuietHours),
                    isOn: $model.quietHoursEnabled
                )
                HStack {
                    TextField(
                        FoundationCopy.text(.notificationSettingsQuietStart),
                        text: $model.quietHoursStart
                    )
                    .accessibilityIdentifier("flit.settings.notifications.quietStart")
                    Text("–")
                    TextField(
                        FoundationCopy.text(.notificationSettingsQuietEnd),
                        text: $model.quietHoursEnd
                    )
                    .accessibilityIdentifier("flit.settings.notifications.quietEnd")
                }
                .disabled(!model.quietHoursEnabled)
                Text(FoundationCopy.text(.notificationSettingsLocalTime))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button(FoundationCopy.text(.notificationSettingsSaveGlobal)) {
                    model.saveGlobal()
                }
                .keyboardShortcut("s", modifiers: [.command])
                .disabled(!model.isLoaded || model.isBusy)
                .accessibilityIdentifier("flit.settings.notifications.saveGlobal")
            }

            Section(FoundationCopy.text(.notificationSettingsProject)) {
                if model.projects.isEmpty {
                    Text(FoundationCopy.text(.notificationSettingsNoProjects))
                        .foregroundStyle(.secondary)
                } else {
                    Picker(
                        FoundationCopy.text(.notificationSettingsProjectPicker),
                        selection: Binding(
                            get: { model.selectedProjectId },
                            set: { model.selectProject($0) }
                        )
                    ) {
                        ForEach(model.projects, id: \.id) { project in
                            Text(project.displayName).tag(Optional(project.id))
                        }
                    }
                    .accessibilityIdentifier("flit.settings.notifications.project")
                    Picker(
                        FoundationCopy.text(.notificationSettingsProjectMaster),
                        selection: $model.projectMaster
                    ) {
                        Text(FoundationCopy.text(.notificationSettingsInherit))
                            .tag(FlitProjectNotificationMaster.inherit)
                        Text(FoundationCopy.text(.notificationSettingsOff))
                            .tag(FlitProjectNotificationMaster.off)
                    }
                    overridePicker(
                        FoundationCopy.text(.notificationSettingsPermission),
                        selection: $model.projectKinds.permission
                    )
                    overridePicker(
                        FoundationCopy.text(.notificationSettingsQuestion),
                        selection: $model.projectKinds.question
                    )
                    overridePicker(
                        FoundationCopy.text(.notificationSettingsFailure),
                        selection: $model.projectKinds.failure
                    )
                    overridePicker(
                        FoundationCopy.text(.notificationSettingsCompletion),
                        selection: $model.projectKinds.completion
                    )
                    overridePicker(
                        FoundationCopy.text(.notificationSettingsStuck),
                        selection: $model.projectKinds.stuck
                    )
                    if let effectiveKinds = model.effectiveKinds {
                        Text(effectiveSummary(effectiveKinds))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .accessibilityIdentifier(
                                "flit.settings.notifications.effective"
                            )
                    }
                    Button(FoundationCopy.text(.notificationSettingsSaveProject)) {
                        model.saveProject()
                    }
                    .disabled(model.selectedProjectId == nil || model.isBusy)
                    .accessibilityIdentifier("flit.settings.notifications.saveProject")
                }
            }
        }
        .formStyle(.grouped)
        .padding(20)
        .frame(minWidth: 620, minHeight: 650)
        .onAppear { model.loadIfNeeded() }
    }

    @ViewBuilder
    private func overridePicker(
        _ title: String,
        selection: Binding<FlitNotificationOverride>
    ) -> some View {
        Picker(title, selection: selection) {
            Text(FoundationCopy.text(.notificationSettingsInherit))
                .tag(FlitNotificationOverride.inherit)
            Text(FoundationCopy.text(.notificationSettingsOn))
                .tag(FlitNotificationOverride.on)
            Text(FoundationCopy.text(.notificationSettingsOff))
                .tag(FlitNotificationOverride.off)
        }
    }

    private func effectiveSummary(_ kinds: FlitNotificationKinds) -> String {
        let enabled = [
            (FoundationCopy.text(.notificationSettingsPermission), kinds.permission),
            (FoundationCopy.text(.notificationSettingsQuestion), kinds.question),
            (FoundationCopy.text(.notificationSettingsFailure), kinds.failure),
            (FoundationCopy.text(.notificationSettingsCompletion), kinds.completion),
            (FoundationCopy.text(.notificationSettingsStuck), kinds.stuck),
        ]
        .filter(\.1)
        .map(\.0)
        .joined(separator: ", ")
        return FoundationCopy.format(.notificationSettingsEffective, enabled)
    }
}

@MainActor
final class NotificationSettingsViewController: NSViewController {
    let model: NotificationSettingsModel
    private var host: NSHostingView<NotificationSettingsView>?

    init(model: NotificationSettingsModel = NotificationSettingsModel()) {
        self.model = model
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func loadView() {
        let host = NSHostingView(rootView: NotificationSettingsView(model: model))
        host.setAccessibilityIdentifier("flit.settings.notifications.root")
        self.host = host
        view = host
    }

    var hostedLeafCount: Int {
        host == nil ? 0 : 1
    }
}
