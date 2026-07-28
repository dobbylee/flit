import AppKit
import Foundation

@MainActor
final class CloseToTrayPreference {
    static let explanationShownKey = "dev.flit.lifecycle.closeToTrayExplanationShown"

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func consumeExplanationIfNeeded() -> Bool {
        if defaults.bool(forKey: Self.explanationShownKey) {
            return false
        }
        defaults.set(true, forKey: Self.explanationShownKey)
        return true
    }
}

struct CloseToTrayAlertContent: Equatable {
    let title: String
    let message: String
    let acknowledgement: String

    static var current: Self {
        Self(
            title: FoundationCopy.text(.closeToTrayTitle),
            message: FoundationCopy.text(.closeToTrayMessage),
            acknowledgement: FoundationCopy.text(.closeToTrayAcknowledgement)
        )
    }
}

@MainActor
protocol CloseToTrayAlertPresenting: AnyObject {
    func present(
        _ content: CloseToTrayAlertContent,
        for window: NSWindow,
        completion: @escaping () -> Void
    )
}

@MainActor
final class AppKitCloseToTrayAlertPresenter: CloseToTrayAlertPresenting {
    func present(
        _ content: CloseToTrayAlertContent,
        for window: NSWindow,
        completion: @escaping () -> Void
    ) {
        let alert = NSAlert()
        alert.alertStyle = .informational
        alert.messageText = content.title
        alert.informativeText = content.message
        alert.addButton(withTitle: content.acknowledgement)
        alert.beginSheetModal(for: window) { _ in completion() }
    }
}

@MainActor
final class ApplicationStatusItemController: NSObject {
    private let openHandler: () -> Void
    private let quitHandler: () -> Void
    let statusItem: NSStatusItem

    init(
        statusBar: NSStatusBar = .system,
        openHandler: @escaping () -> Void,
        quitHandler: @escaping () -> Void
    ) {
        self.openHandler = openHandler
        self.quitHandler = quitHandler
        statusItem = statusBar.statusItem(withLength: NSStatusItem.variableLength)
        super.init()

        statusItem.button?.title = "Flit"
        statusItem.button?.toolTip = FoundationCopy.text(.menuBarTooltip)
        statusItem.button?.setAccessibilityIdentifier("flit.statusItem")

        let menu = NSMenu()
        let openItem = NSMenuItem(
            title: FoundationCopy.text(.menuOpen),
            action: #selector(openFlit(_:)),
            keyEquivalent: ""
        )
        openItem.target = self
        openItem.identifier = NSUserInterfaceItemIdentifier("flit.statusItem.open")
        menu.addItem(openItem)
        menu.addItem(.separator())
        let quitItem = NSMenuItem(
            title: FoundationCopy.text(.menuQuit),
            action: #selector(quitFlit(_:)),
            keyEquivalent: ""
        )
        quitItem.target = self
        quitItem.identifier = NSUserInterfaceItemIdentifier("flit.statusItem.quit")
        menu.addItem(quitItem)
        statusItem.menu = menu
    }

    @objc
    func openFlit(_ sender: Any?) {
        openHandler()
    }

    @objc
    func quitFlit(_ sender: Any?) {
        quitHandler()
    }
}
