import AppKit
import Foundation

enum ExplicitQuitPreview: Equatable {
    case exact(FlitQuitImpactResponse)
    case unavailable
}

struct QuitImpactClient: Sendable {
    let clientProtocolVersion: String

    init(clientProtocolVersion: String = flitClientProtocolVersion) {
        self.clientProtocolVersion = clientProtocolVersion
    }

    func load() -> ExplicitQuitPreview {
        do {
            let rendered = try quitImpactJson(clientProtocolVersion: clientProtocolVersion)
            let response = try JSONDecoder().decode(
                FlitQuitImpactResponse.self,
                from: Data(rendered.utf8)
            )
            guard
                response.protocolVersion == clientProtocolVersion,
                !response.coreInstanceId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                response.flitMonitoringStops,
                response.flitNotificationsStop,
                Set(response.runs.map(\.runId)).count == response.runs.count,
                response.runs.allSatisfy({
                    !$0.runId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        && !$0.title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                })
            else {
                return .unavailable
            }
            return .exact(response)
        } catch {
            return .unavailable
        }
    }
}

struct ExplicitQuitAlertContent: Equatable {
    let title: String
    let message: String
    let confirmTitle: String
    let cancelTitle: String

    static func make(for preview: ExplicitQuitPreview) -> Self {
        let impact: String
        switch preview {
        case let .exact(response):
            if response.runs.isEmpty {
                impact = FoundationCopy.text(.quitNoActiveRuns)
            } else {
                let lines = response.runs.map { run in
                    switch run.executionAfterQuit {
                    case .continues:
                        FoundationCopy.format(
                            .quitRunContinues,
                            run.title,
                            providerName(run.provider)
                        )
                    case .stops:
                        FoundationCopy.format(
                            .quitRunStops,
                            run.title,
                            providerName(run.provider)
                        )
                    case .unknown:
                        FoundationCopy.format(
                            .quitRunUnknown,
                            run.title,
                            providerName(run.provider)
                        )
                    }
                }
                impact = ([FoundationCopy.text(.quitActiveRuns)] + lines)
                    .joined(separator: "\n")
            }
        case .unavailable:
            impact = FoundationCopy.text(.quitImpactUnavailable)
        }
        return Self(
            title: FoundationCopy.text(.quitTitle),
            message: "\(impact)\n\n\(FoundationCopy.text(.quitMonitoringBoundary))",
            confirmTitle: FoundationCopy.text(.quitConfirm),
            cancelTitle: FoundationCopy.text(.quitCancel)
        )
    }

    private static func providerName(_ provider: FlitProviderKind) -> String {
        switch provider {
        case .codex:
            "Codex"
        }
    }
}

enum ExplicitQuitChoice: Equatable {
    case confirm
    case cancel
}

enum ExplicitQuitDisposition: Equatable {
    case terminateNow
    case pending
}

@MainActor
protocol ExplicitQuitAlertPresenting: AnyObject {
    func present(
        _ content: ExplicitQuitAlertContent,
        for window: NSWindow,
        completion: @escaping (ExplicitQuitChoice) -> Void
    )
}

@MainActor
final class AppKitExplicitQuitAlertPresenter: ExplicitQuitAlertPresenting {
    func present(
        _ content: ExplicitQuitAlertContent,
        for window: NSWindow,
        completion: @escaping (ExplicitQuitChoice) -> Void
    ) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = content.title
        alert.informativeText = content.message
        alert.addButton(withTitle: content.confirmTitle)
        alert.addButton(withTitle: content.cancelTitle)
        alert.buttons.first?.hasDestructiveAction = true
        alert.beginSheetModal(for: window) { response in
            completion(response == .alertFirstButtonReturn ? .confirm : .cancel)
        }
    }
}

@MainActor
final class ExplicitQuitCoordinator {
    private let previewLoader: () -> ExplicitQuitPreview
    private let presenter: any ExplicitQuitAlertPresenting
    private var confirmationInFlight = false

    init(
        previewLoader: @escaping () -> ExplicitQuitPreview = QuitImpactClient().load,
        presenter: any ExplicitQuitAlertPresenting = AppKitExplicitQuitAlertPresenter()
    ) {
        self.previewLoader = previewLoader
        self.presenter = presenter
    }

    func requestQuit(
        for window: NSWindow,
        completion: @escaping (Bool) -> Void
    ) -> ExplicitQuitDisposition {
        guard !confirmationInFlight else { return .pending }
        let preview = previewLoader()
        if case let .exact(response) = preview, response.runs.isEmpty {
            return .terminateNow
        }
        present(preview, for: window, completion: completion)
        return .pending
    }

    private func present(
        _ preview: ExplicitQuitPreview,
        for window: NSWindow,
        completion: @escaping (Bool) -> Void
    ) {
        confirmationInFlight = true
        presenter.present(.make(for: preview), for: window) { [weak self] choice in
            guard let self else { return }
            self.confirmationInFlight = false
            guard choice == .confirm else {
                completion(false)
                return
            }
            let current = self.previewLoader()
            if current == preview {
                completion(true)
            } else {
                self.present(current, for: window, completion: completion)
            }
        }
    }
}
