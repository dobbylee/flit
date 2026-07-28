import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private var windowController: NSWindowController?
    private var statusItemController: ApplicationStatusItemController?
    private let closeToTrayPreference: CloseToTrayPreference
    private let closeToTrayAlertPresenter: any CloseToTrayAlertPresenting
    private let dataDirectoryProvider: @MainActor () -> String

    init(
        closeToTrayPreference: CloseToTrayPreference = CloseToTrayPreference(),
        closeToTrayAlertPresenter: any CloseToTrayAlertPresenting =
            AppKitCloseToTrayAlertPresenter(),
        dataDirectoryProvider: @escaping @MainActor () -> String =
            AppDelegate.defaultDataDirectory
    ) {
        self.closeToTrayPreference = closeToTrayPreference
        self.closeToTrayAlertPresenter = closeToTrayAlertPresenter
        self.dataDirectoryProvider = dataDirectoryProvider
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
            quitHandler: {
                NSApplication.shared.terminate(nil)
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
        applicationMenu.addItem(
            withTitle: FoundationCopy.text(.menuQuit),
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        applicationItem.submenu = applicationMenu
        NSApplication.shared.mainMenu = mainMenu
    }

    #if FLIT_NATIVE_TESTS
        var testMainWindow: NSWindow? {
            windowController?.window
        }

        func testOpenFromStatusItem() {
            statusItemController?.openFlit(nil)
        }
    #endif
}
