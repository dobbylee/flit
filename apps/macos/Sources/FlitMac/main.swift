import AppKit

@MainActor
func runFlitApplication() {
    let application = NSApplication.shared
    let delegate = AppDelegate()
    application.delegate = delegate
    application.setActivationPolicy(.regular)
    application.run()
}

runFlitApplication()
