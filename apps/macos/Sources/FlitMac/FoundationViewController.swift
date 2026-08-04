import AppKit
import SwiftUI

private final class FlippedDashboardDocumentView: NSView {
    override var isFlipped: Bool { true }
}

private final class RunDetailButton: NSButton {
    let runId: String
    let runVersion: UInt64
    let runTitle: String

    init(runId: String, runVersion: UInt64, runTitle: String) {
        self.runId = runId
        self.runVersion = runVersion
        self.runTitle = runTitle
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }
}

@MainActor
final class FoundationViewController: NSViewController {
    private let client: SystemHealthClient
    private let dashboardClient: DashboardClient
    private let runDetailClient: RunDetailClient
    private var state: FoundationState = .checking
    private var dashboardState = DashboardPresentationState()
    private var statusHost: NSHostingView<FoundationStatusBadge>?
    private var boundaryLabel: NSTextField?
    private var foundationPanel: NSStackView?
    private var dashboardStack: NSStackView?
    private var dashboardScroll: NSScrollView?

    init(
        client: SystemHealthClient,
        dashboardClient: DashboardClient = DashboardClient(),
        runDetailClient: RunDetailClient = RunDetailClient()
    ) {
        self.client = client
        self.dashboardClient = dashboardClient
        self.runDetailClient = runDetailClient
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

        let dashboard = NSStackView()
        identify(dashboard, as: "flit.dashboard.sections")
        dashboard.orientation = .vertical
        dashboard.alignment = .leading
        dashboard.spacing = 12
        dashboard.translatesAutoresizingMaskIntoConstraints = false
        dashboardStack = dashboard

        let dashboardDocument = FlippedDashboardDocumentView()
        dashboardDocument.addSubview(dashboard)
        let dashboardScroll = NSScrollView()
        identify(dashboardScroll, as: "flit.dashboard.scroll")
        dashboardScroll.borderType = .noBorder
        dashboardScroll.drawsBackground = false
        dashboardScroll.hasHorizontalScroller = false
        dashboardScroll.hasVerticalScroller = true
        dashboardScroll.documentView = dashboardDocument
        dashboardScroll.translatesAutoresizingMaskIntoConstraints = false
        self.dashboardScroll = dashboardScroll
        let preferredDashboardHeight = dashboardScroll.heightAnchor.constraint(
            equalToConstant: 206
        )
        preferredDashboardHeight.priority = .defaultHigh
        NSLayoutConstraint.activate([
            dashboard.widthAnchor.constraint(equalTo: dashboardDocument.widthAnchor),
            dashboard.topAnchor.constraint(equalTo: dashboardDocument.topAnchor),
            dashboard.leadingAnchor.constraint(equalTo: dashboardDocument.leadingAnchor),
            dashboard.trailingAnchor.constraint(equalTo: dashboardDocument.trailingAnchor),
            dashboard.bottomAnchor.constraint(equalTo: dashboardDocument.bottomAnchor),
            dashboardDocument.widthAnchor.constraint(equalTo: dashboardScroll.contentView.widthAnchor),
            dashboardScroll.widthAnchor.constraint(equalToConstant: 528),
            dashboardScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 32),
            preferredDashboardHeight,
        ])

        [mark, phase, title, summary, host, boundary, dashboardScroll, footer].forEach {
            arrangedView in
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
        availableWidth.priority = NSLayoutConstraint.Priority(999)
        NSLayoutConstraint.activate([
            panel.centerXAnchor.constraint(equalTo: root.centerXAnchor),
            panel.centerYAnchor.constraint(equalTo: root.centerYAnchor),
            panel.topAnchor.constraint(greaterThanOrEqualTo: root.topAnchor, constant: 24),
            panel.bottomAnchor.constraint(lessThanOrEqualTo: root.bottomAnchor, constant: -24),
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
            loadDashboard()
        case .unavailable:
            state = .unavailable
            renderDashboardFailure(FoundationCopy.text(.dashboardUnavailable))
        }
        statusHost?.rootView = FoundationStatusBadge(state: state)
        boundaryLabel?.stringValue = FoundationCopy.text(state.boundaryCopy)
    }

    private func loadDashboard() {
        do {
            try dashboardState.apply(dashboardClient.loadInitial())
            renderDashboard()
        } catch {
            renderDashboardFailure(FoundationCopy.text(.dashboardUnavailable))
        }
    }

    private func renderDashboard() {
        guard let dashboardStack else { return }
        dashboardStack.arrangedSubviews.forEach { view in
            dashboardStack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        for section in DashboardSection.allCases {
            let heading = label(section.title, size: 14, weight: .semibold)
            identify(heading, as: "flit.dashboard.section.\(section.rawValue)")
            dashboardStack.addArrangedSubview(heading)
            do {
                let runs = try dashboardState.runs(in: section)
                if runs.isEmpty {
                    let empty = label(
                        FoundationCopy.text(.dashboardNoRuns),
                        size: 12,
                        weight: .regular,
                        color: .secondaryLabelColor
                    )
                    dashboardStack.addArrangedSubview(empty)
                } else {
                    for run in runs {
                        dashboardStack.addArrangedSubview(runCard(run))
                    }
                }
            } catch {
                renderDashboardFailure(FoundationCopy.text(.dashboardDataUnavailable))
                return
            }
        }
    }

    private func renderDashboardFailure(_ message: String) {
        guard let dashboardStack else { return }
        dashboardStack.arrangedSubviews.forEach { view in
            dashboardStack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        let failure = label(message, size: 13, weight: .semibold, color: .systemRed)
        identify(failure, as: "flit.dashboard.unavailable")
        dashboardStack.addArrangedSubview(failure)
    }

    private func runCard(_ run: FlitDashboardRunRecord) -> NSView {
        let card = NSStackView()
        identify(card, as: "flit.dashboard.run.\(run.runId)")
        card.orientation = .vertical
        card.alignment = .leading
        card.spacing = 4
        card.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
        card.wantsLayer = true
        card.layer?.cornerRadius = 10
        card.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        card.addArrangedSubview(label(run.title, size: 14, weight: .semibold))
        card.addArrangedSubview(
            label(
                "\(run.projectDisplayName) · \(run.provider.rawValue) · \(run.lifecycle)",
                size: 12,
                weight: .regular,
                color: .secondaryLabelColor
            )
        )
        let activity = run.activity == "Unknown"
            ? FoundationCopy.text(.dashboardActivityUnknown)
            : FoundationCopy.format(
                .dashboardActivity,
                run.activity,
                Int(run.activityConfidence * 100)
            )
        card.addArrangedSubview(label(activity, size: 12, weight: .regular))
        card.addArrangedSubview(
            label(
                FoundationCopy.format(
                    .dashboardAttention,
                    run.attentionLevel,
                    run.attentionOpenCount
                ),
                size: 12,
                weight: .regular
            )
        )
        let changes: String
        switch run.changes {
        case let .available(attribution, files, insertions, deletions):
            changes = FoundationCopy.format(
                attribution == .exact
                    ? .dashboardChanges
                    : .dashboardChangesObservedDuringRun,
                files,
                insertions,
                deletions
            )
        case let .unavailable(reason):
            changes = FoundationCopy.format(.dashboardChangesUnavailable, reason)
        }
        card.addArrangedSubview(
            label(changes, size: 12, weight: .regular, color: .secondaryLabelColor)
        )
        let detail = RunDetailButton(
            runId: run.runId,
            runVersion: run.version,
            runTitle: run.title
        )
        detail.title = FoundationCopy.text(.dashboardViewActivity)
        detail.bezelStyle = .inline
        detail.target = self
        detail.action = #selector(showRunDetail(_:))
        identify(detail, as: "flit.dashboard.runDetail.\(run.runId)")
        card.addArrangedSubview(detail)
        return card
    }

    @objc private func showRunDetail(_ sender: RunDetailButton) {
        do {
            let response = try runDetailClient.loadFirstPage(
                runId: sender.runId,
                expectedRunVersion: sender.runVersion
            )
            var detail = RunDetailPresentationState()
            try detail.apply(
                response,
                requestedRunId: sender.runId,
                expectedRunVersion: sender.runVersion,
                requestedAfterCursor: 0,
                requestedEventLimit: 50
            )
            renderRunDetail(detail, runTitle: sender.runTitle)
        } catch {
            renderRunDetailFailure()
        }
    }

    @objc private func showDashboard(_: NSButton) {
        renderDashboard()
    }

    private func renderRunDetail(
        _ detail: RunDetailPresentationState,
        runTitle: String
    ) {
        guard
            let dashboardStack,
            let runId = detail.runId,
            let historyStatus = detail.historyStatus,
            let openInProviderStatus = detail.openInProviderStatus
        else {
            renderRunDetailFailure()
            return
        }
        clearDashboardStack()
        dashboardStack.addArrangedSubview(backButton())
        let heading = label(
            FoundationCopy.format(.runDetailTitle, runTitle),
            size: 16,
            weight: .semibold
        )
        identify(heading, as: "flit.runDetail.title.\(runId)")
        dashboardStack.addArrangedSubview(heading)
        dashboardStack.addArrangedSubview(
            label(
                FoundationCopy.format(
                    .runDetailCapability,
                    FoundationCopy.text(.runDetailProviderHistory),
                    historyStatus.rawValue
                ),
                size: 12,
                weight: .regular,
                color: .secondaryLabelColor
            )
        )
        dashboardStack.addArrangedSubview(
            label(
                FoundationCopy.format(
                    .runDetailCapability,
                    FoundationCopy.text(.runDetailOpenInProvider),
                    openInProviderStatus.rawValue
                ),
                size: 12,
                weight: .regular,
                color: .secondaryLabelColor
            )
        )
        if detail.events.isEmpty {
            dashboardStack.addArrangedSubview(
                label(
                    FoundationCopy.text(.runDetailNoEvents),
                    size: 12,
                    weight: .regular,
                    color: .secondaryLabelColor
                )
            )
        } else {
            for event in detail.events {
                let row = label(
                    FoundationCopy.format(
                        .runDetailEvent,
                        event.observedAt,
                        event.eventType,
                        event.sourceKind.rawValue,
                        Int(event.confidence * 100)
                    ),
                    size: 12,
                    weight: .regular
                )
                identify(row, as: "flit.runDetail.event.\(event.cursor)")
                dashboardStack.addArrangedSubview(row)
            }
        }
        if detail.hasMore {
            dashboardStack.addArrangedSubview(
                label(
                    FoundationCopy.text(.runDetailMoreEvents),
                    size: 12,
                    weight: .semibold,
                    color: .secondaryLabelColor
                )
            )
        }
        scrollDashboardToTop()
    }

    private func renderRunDetailFailure() {
        guard let dashboardStack else { return }
        clearDashboardStack()
        dashboardStack.addArrangedSubview(backButton())
        let failure = label(
            FoundationCopy.text(.runDetailUnavailable),
            size: 13,
            weight: .semibold,
            color: .systemRed
        )
        identify(failure, as: "flit.runDetail.unavailable")
        dashboardStack.addArrangedSubview(failure)
        scrollDashboardToTop()
    }

    private func backButton() -> NSButton {
        let button = NSButton(
            title: FoundationCopy.text(.runDetailBack),
            target: self,
            action: #selector(showDashboard(_:))
        )
        button.bezelStyle = .inline
        identify(button, as: "flit.runDetail.back")
        return button
    }

    private func clearDashboardStack() {
        guard let dashboardStack else { return }
        dashboardStack.arrangedSubviews.forEach { view in
            dashboardStack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
    }

    private func scrollDashboardToTop() {
        guard let dashboardScroll else { return }
        dashboardScroll.documentView?.layoutSubtreeIfNeeded()
        dashboardScroll.contentView.scroll(to: .zero)
        dashboardScroll.reflectScrolledClipView(dashboardScroll.contentView)
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
