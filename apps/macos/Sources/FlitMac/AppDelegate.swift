import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private var windowController: NSWindowController?
    private var statusItemController: ApplicationStatusItemController?
    private let closeToTrayPreference: CloseToTrayPreference
    private let closeToTrayAlertPresenter: any CloseToTrayAlertPresenting
    private let dataDirectoryProvider: @MainActor () -> String
    private let explicitQuitCoordinator: ExplicitQuitCoordinator
    private let applicationTerminator: @MainActor () -> Void
    private let terminationReplyHandler: @MainActor (NSApplication, Bool) -> Void
    private var terminationReplyPending = false

    init(
        closeToTrayPreference: CloseToTrayPreference = CloseToTrayPreference(),
        closeToTrayAlertPresenter: any CloseToTrayAlertPresenting =
            AppKitCloseToTrayAlertPresenter(),
        dataDirectoryProvider: @escaping @MainActor () -> String =
            AppDelegate.defaultDataDirectory,
        explicitQuitCoordinator: ExplicitQuitCoordinator = ExplicitQuitCoordinator(),
        applicationTerminator: @escaping @MainActor () -> Void = {
            NSApplication.shared.terminate(nil)
        },
        terminationReplyHandler: @escaping @MainActor (NSApplication, Bool) -> Void = {
            application, shouldTerminate in
            application.reply(toApplicationShouldTerminate: shouldTerminate)
        }
    ) {
        self.closeToTrayPreference = closeToTrayPreference
        self.closeToTrayAlertPresenter = closeToTrayAlertPresenter
        self.dataDirectoryProvider = dataDirectoryProvider
        self.explicitQuitCoordinator = explicitQuitCoordinator
        self.applicationTerminator = applicationTerminator
        self.terminationReplyHandler = terminationReplyHandler
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        configureMainMenu()
        configureStatusItem()
        showMainWindow()
        NSApplication.shared.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldTerminate(
        _ sender: NSApplication
    ) -> NSApplication.TerminateReply {
        guard !terminationReplyPending else { return .terminateLater }
        showMainWindow()
        NSApplication.shared.activate(ignoringOtherApps: true)
        guard let window = windowController?.window else { return .terminateCancel }

        let disposition = explicitQuitCoordinator.requestQuit(for: window) {
            [weak self, weak sender] shouldTerminate in
            guard let self, let sender else { return }
            self.terminationReplyPending = false
            self.terminationReplyHandler(sender, shouldTerminate)
        }
        switch disposition {
        case .terminateNow:
            return .terminateNow
        case .pending:
            terminationReplyPending = true
            return .terminateLater
        }
    }

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        if !flag {
            showMainWindow()
        }
        return true
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if closeToTrayPreference.consumeExplanationIfNeeded() {
            closeToTrayAlertPresenter.present(
                .current,
                for: sender
            ) { [weak sender] in
                sender?.orderOut(nil)
            }
        } else {
            sender.orderOut(nil)
        }
        return false
    }

    private func showMainWindow() {
        if let window = windowController?.window {
            window.makeKeyAndOrderFront(nil)
            return
        }

        let client = SystemHealthClient()
        do {
            try initializeCore(
                dataDirectory: dataDirectoryProvider(),
                clientProtocolVersion: client.clientProtocolVersion
            )
        } catch {
            // The generated health command reports the failed initialization without path details.
        }
        let content = FoundationViewController(client: client)
        let window = NSWindow(contentViewController: content)
        window.title = "Flit"
        window.identifier = NSUserInterfaceItemIdentifier("flit.mainWindow")
        window.setAccessibilityIdentifier("flit.mainWindow")
        window.setContentSize(NSSize(width: 1_280, height: 720))
        window.minSize = NSSize(width: 720, height: 560)
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.delegate = self
        window.center()

        let controller = NSWindowController(window: window)
        windowController = controller
        controller.showWindow(nil)
    }

    private func configureStatusItem() {
        statusItemController = ApplicationStatusItemController(
            openHandler: { [weak self] in
                self?.showMainWindow()
                NSApplication.shared.activate(ignoringOtherApps: true)
            },
            quitHandler: { [weak self] in
                self?.requestExplicitQuit(nil)
            }
        )
    }

    private static func defaultDataDirectory() -> String {
        guard
            let applicationSupport = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            ).first
        else {
            return ""
        }
        return applicationSupport.appendingPathComponent("Flit", isDirectory: true).path
    }

    private func configureMainMenu() {
        let mainMenu = NSMenu()
        let applicationItem = NSMenuItem()
        mainMenu.addItem(applicationItem)

        let applicationMenu = NSMenu()
        let quitItem = NSMenuItem(
            title: FoundationCopy.text(.menuQuit),
            action: #selector(requestExplicitQuit(_:)),
            keyEquivalent: "q"
        )
        quitItem.target = self
        quitItem.identifier = NSUserInterfaceItemIdentifier("flit.mainMenu.quit")
        applicationMenu.addItem(quitItem)
        applicationItem.submenu = applicationMenu
        NSApplication.shared.mainMenu = mainMenu
    }

    @objc
    private func requestExplicitQuit(_ sender: Any?) {
        applicationTerminator()
    }

    #if FLIT_NATIVE_TESTS
        var testMainWindow: NSWindow? {
            windowController?.window
        }

        func testOpenFromStatusItem() {
            statusItemController?.openFlit(nil)
        }

        func testQuitFromStatusItem() {
            statusItemController?.quitFlit(nil)
        }
    #endif
}
