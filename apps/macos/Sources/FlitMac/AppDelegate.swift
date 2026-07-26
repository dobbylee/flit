import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var windowController: NSWindowController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        configureMainMenu()
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

    private func showMainWindow() {
        if let window = windowController?.window {
            window.makeKeyAndOrderFront(nil)
            return
        }

        let client = SystemHealthClient()
        do {
            try initializeCore(
                dataDirectory: applicationDataDirectory(),
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
        window.center()

        let controller = NSWindowController(window: window)
        windowController = controller
        controller.showWindow(nil)
    }

    private func applicationDataDirectory() -> String {
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
            withTitle: "Quit Flit",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        applicationItem.submenu = applicationMenu
        NSApplication.shared.mainMenu = mainMenu
    }
}
