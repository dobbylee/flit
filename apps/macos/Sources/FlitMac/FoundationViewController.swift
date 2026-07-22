import AppKit
import SwiftUI

@MainActor
final class FoundationViewController: NSViewController {
    private let client: SystemHealthClient
    private var state: FoundationState = .checking
    private var statusHost: NSHostingView<FoundationStatusBadge>?
    private var boundaryLabel: NSTextField?
    private var foundationPanel: NSStackView?

    init(client: SystemHealthClient = SystemHealthClient()) {
        self.client = client
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func loadView() {
        let root = NSView()
        identify(root, as: "flit.foundation.root")
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        let panel = NSStackView()
        identify(panel, as: "flit.foundation.panel")
        panel.orientation = .vertical
        panel.alignment = .leading
        panel.spacing = 18
        panel.edgeInsets = NSEdgeInsets(top: 42, left: 48, bottom: 42, right: 48)
        panel.translatesAutoresizingMaskIntoConstraints = false
        panel.wantsLayer = true
        panel.layer?.cornerRadius = 24
        panel.layer?.borderWidth = 1
        panel.layer?.borderColor = NSColor.separatorColor.cgColor
        panel.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        foundationPanel = panel

        let mark = label("F", size: 24, weight: .bold, color: .white)
        mark.alignment = .center
        mark.wantsLayer = true
        mark.layer?.cornerRadius = 12
        mark.layer?.backgroundColor = NSColor.systemGreen.withAlphaComponent(0.75).cgColor
        mark.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            mark.widthAnchor.constraint(equalToConstant: 44),
            mark.heightAnchor.constraint(equalToConstant: 44),
        ])

        let phase = label(FoundationCopy.text(.phase), size: 12, weight: .semibold)
        identify(phase, as: "flit.foundation.phase")
        let title = label(FoundationCopy.text(.title), size: 48, weight: .medium)
        identify(title, as: "flit.foundation.title")
        let summary = label(FoundationCopy.text(.summary), size: 18, weight: .regular)
        identify(summary, as: "flit.foundation.summary")
        summary.maximumNumberOfLines = 2

        let host = NSHostingView(rootView: FoundationStatusBadge(state: state))
        identify(host, as: "flit.foundation.statusHost")
        host.translatesAutoresizingMaskIntoConstraints = false
        host.heightAnchor.constraint(greaterThanOrEqualToConstant: 30).isActive = true
        statusHost = host

        let boundary = label(FoundationCopy.text(state.boundaryCopy), size: 14, weight: .regular)
        identify(boundary, as: "flit.foundation.boundary")
        boundary.maximumNumberOfLines = 3
        boundaryLabel = boundary

        let footer = label(
            "\(FoundationCopy.text(.local))  ·  \(FoundationCopy.text(.noControls))",
            size: 12,
            weight: .regular,
            color: .secondaryLabelColor
        )
        identify(footer, as: "flit.foundation.footer")

        [mark, phase, title, summary, host, boundary, footer].forEach { arrangedView in
            panel.addArrangedSubview(arrangedView)
        }
        panel.setCustomSpacing(30, after: mark)
        panel.setCustomSpacing(28, after: summary)
        panel.setCustomSpacing(26, after: boundary)

        root.addSubview(panel)
        let availableWidth = panel.widthAnchor.constraint(
            equalTo: root.widthAnchor,
            constant: -96
        )
        availableWidth.priority = NSLayoutConstraint.Priority(749)
        NSLayoutConstraint.activate([
            panel.centerXAnchor.constraint(equalTo: root.centerXAnchor),
            panel.centerYAnchor.constraint(equalTo: root.centerYAnchor),
            panel.widthAnchor.constraint(lessThanOrEqualToConstant: 680),
            panel.widthAnchor.constraint(lessThanOrEqualTo: root.widthAnchor, constant: -96),
            availableWidth,
        ])

        view = root
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        refresh()
    }

    var hostedLeafCount: Int {
        statusHost == nil ? 0 : 1
    }

    var currentState: FoundationState {
        state
    }

    var foundationPanelFrame: NSRect? {
        foundationPanel?.frame
    }

    var hasAmbiguousFoundationLayout: Bool {
        view.hasAmbiguousLayout || foundationPanel?.hasAmbiguousLayout != false
    }

    private func refresh() {
        switch client.load() {
        case .ready:
            state = .ready
        case .unavailable:
            state = .unavailable
        }
        statusHost?.rootView = FoundationStatusBadge(state: state)
        boundaryLabel?.stringValue = FoundationCopy.text(state.boundaryCopy)
    }

    private func label(
        _ text: String,
        size: CGFloat,
        weight: NSFont.Weight,
        color: NSColor = .labelColor
    ) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = NSFont.systemFont(ofSize: size, weight: weight)
        field.textColor = color
        field.lineBreakMode = .byWordWrapping
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    private func identify(_ view: NSView, as value: String) {
        view.identifier = NSUserInterfaceItemIdentifier(value)
        view.setAccessibilityIdentifier(value)
    }
}
