import AppKit
import SwiftUI

private final class FlippedDashboardDocumentView: NSView {
    override var isFlipped: Bool { true }
}

private final class RunDetailButton: NSButton {
    let run: FlitDashboardRunRecord

    init(run: FlitDashboardRunRecord) {
        self.run = run
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }
}

private final class RunEvidenceButton: NSButton {
    let runId: String
    let eventId: String
    let cursor: UInt64

    init(runId: String, eventId: String, cursor: UInt64) {
        self.runId = runId
        self.eventId = eventId
        self.cursor = cursor
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }
}

private func dashboardChangesCopy(_ changes: FlitDashboardChangeSummary) -> String {
    switch changes {
    case let .available(attribution, files, insertions, deletions):
        FoundationCopy.format(
            attribution == .exact
                ? .dashboardChanges
                : .dashboardChangesObservedDuringRun,
            files,
            insertions,
            deletions
        )
    case let .unavailable(reason):
        FoundationCopy.format(.dashboardChangesUnavailable, reason)
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
    private var activeRunDetail: RunDetailPresentationState?
    private var activeRunTitle: String?
    private var activeRunCompletionSummary: RunCompletionSummary?
    private var activeRunDetailFilter = RunActivityFilter.all
    private var activeRunDetailPageFailure = false
    private var expandedRunEvidenceIds: Set<String> = []
    private var runEvidenceButtonsByCursor: [UInt64: NSButton] = [:]

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
        card.addArrangedSubview(
            label(
                dashboardChangesCopy(run.changes),
                size: 12,
                weight: .regular,
                color: .secondaryLabelColor
            )
        )
        let detail = RunDetailButton(run: run)
        detail.title = FoundationCopy.text(.dashboardViewActivity)
        detail.bezelStyle = .inline
        detail.target = self
        detail.action = #selector(showRunDetail(_:))
        identify(detail, as: "flit.dashboard.runDetail.\(run.runId)")
        card.addArrangedSubview(detail)
        return card
    }

    @objc private func showRunDetail(_ sender: RunDetailButton) {
        activeRunDetailFilter = .all
        activeRunDetailPageFailure = false
        expandedRunEvidenceIds = []
        do {
            let completionSummary = try runCompletionSummary(for: sender.run)
            let response = try runDetailClient.loadFirstPage(
                runId: sender.run.runId,
                expectedRunVersion: sender.run.version
            )
            var detail = RunDetailPresentationState()
            try detail.apply(
                response,
                requestedRunId: sender.run.runId,
                expectedRunVersion: sender.run.version,
                requestedAfterCursor: 0,
                requestedEventLimit: 50
            )
            activeRunDetail = detail
            activeRunTitle = sender.run.title
            activeRunCompletionSummary = completionSummary
            renderRunDetail(detail, runTitle: sender.run.title, pageFailure: false)
        } catch {
            activeRunDetail = nil
            activeRunTitle = nil
            activeRunCompletionSummary = nil
            renderRunDetailFailure()
        }
    }

    @objc private func showDashboard(_: NSButton) {
        activeRunDetail = nil
        activeRunTitle = nil
        activeRunCompletionSummary = nil
        activeRunDetailFilter = .all
        activeRunDetailPageFailure = false
        expandedRunEvidenceIds = []
        renderDashboard()
    }

    @objc private func loadMoreRunDetail(_: NSButton) {
        guard
            var detail = activeRunDetail,
            let runId = detail.runId,
            let runVersion = detail.runVersion,
            let runTitle = activeRunTitle
        else {
            renderRunDetailFailure()
            return
        }
        let afterCursor = detail.nextCursor
        do {
            let response = try runDetailClient.loadPage(
                runId: runId,
                expectedRunVersion: runVersion,
                afterCursor: afterCursor
            )
            try detail.append(
                response,
                requestedRunId: runId,
                expectedRunVersion: runVersion,
                requestedAfterCursor: afterCursor,
                requestedEventLimit: 50
            )
            activeRunDetail = detail
            activeRunDetailPageFailure = false
            renderRunDetail(detail, runTitle: runTitle, pageFailure: false)
        } catch {
            activeRunDetailPageFailure = true
            renderRunDetail(detail, runTitle: runTitle, pageFailure: true)
        }
    }

    @objc private func changeRunDetailFilter(_ sender: NSPopUpButton) {
        guard
            RunActivityFilter.allCases.indices.contains(sender.indexOfSelectedItem),
            let detail = activeRunDetail,
            let runTitle = activeRunTitle
        else {
            renderRunDetailFailure()
            return
        }
        activeRunDetailFilter = RunActivityFilter.allCases[sender.indexOfSelectedItem]
        renderRunDetail(
            detail,
            runTitle: runTitle,
            pageFailure: activeRunDetailPageFailure
        )
    }

    @objc private func toggleRunEvidence(_ sender: RunEvidenceButton) {
        guard
            let detail = activeRunDetail,
            let runTitle = activeRunTitle,
            detail.runId == sender.runId,
            detail.events.contains(where: {
                $0.eventId == sender.eventId && $0.cursor == sender.cursor
            })
        else {
            return
        }
        if !expandedRunEvidenceIds.insert(sender.eventId).inserted {
            expandedRunEvidenceIds.remove(sender.eventId)
        }
        renderRunDetail(
            detail,
            runTitle: runTitle,
            pageFailure: activeRunDetailPageFailure,
            focusedEvidenceCursor: sender.cursor
        )
    }

    private func renderRunDetail(
        _ detail: RunDetailPresentationState,
        runTitle: String,
        pageFailure: Bool,
        focusedEvidenceCursor: UInt64? = nil
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
        if let completionSummary = activeRunCompletionSummary {
            dashboardStack.addArrangedSubview(
                runCompletionSummaryView(completionSummary)
            )
        }
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
        let openInProvider = NSButton(
            title: FoundationCopy.text(.runDetailOpenInProvider),
            target: nil,
            action: nil
        )
        openInProvider.bezelStyle = .inline
        openInProvider.isEnabled = false
        identify(openInProvider, as: "flit.runDetail.openInProvider")
        dashboardStack.addArrangedSubview(openInProvider)
        let openInProviderReason = label(
            FoundationCopy.providerOpenUnavailableReason(openInProviderStatus),
            size: 11,
            weight: .regular,
            color: .secondaryLabelColor
        )
        identify(openInProviderReason, as: "flit.runDetail.openInProvider.reason")
        dashboardStack.addArrangedSubview(openInProviderReason)
        dashboardStack.addArrangedSubview(runDetailFilterControl())
        let visibleGroups = activeRunDetailFilter.visibleGroups(in: detail.events)
        if visibleGroups.isEmpty {
            let emptyCopy: String
            let emptyIdentifier: String
            if activeRunDetailFilter == .all {
                emptyCopy = FoundationCopy.text(.runDetailNoEvents)
                emptyIdentifier = "flit.runDetail.noEvents"
            } else {
                emptyCopy = FoundationCopy.format(
                    detail.hasMore
                        ? .runDetailNoMatchingLoadedEvents
                        : .runDetailNoMatchingEvents,
                    runDetailFilterTitle(activeRunDetailFilter)
                )
                emptyIdentifier =
                    "flit.runDetail.\(detail.hasMore ? "noMatchingLoadedEvents" : "noMatchingEvents").\(runDetailFilterIdentifier(activeRunDetailFilter))"
            }
            let empty = label(
                emptyCopy,
                size: 12,
                weight: .regular,
                color: .secondaryLabelColor
            )
            identify(empty, as: emptyIdentifier)
            dashboardStack.addArrangedSubview(
                empty
            )
        } else {
            for group in visibleGroups {
                if group.events.count == 1 {
                    dashboardStack.addArrangedSubview(
                        runDetailEventView(group.events[0], runId: runId)
                    )
                } else {
                    let hasUnloadedTail = detail.hasMore
                        && group.events.last?.cursor == detail.events.last?.cursor
                    dashboardStack.addArrangedSubview(
                        runDetailGroup(
                            group,
                            runId: runId,
                            hasUnloadedTail: hasUnloadedTail
                        )
                    )
                }
            }
        }
        if detail.hasMore {
            let loadMore = NSButton(
                title: FoundationCopy.text(.runDetailLoadMore),
                target: self,
                action: #selector(loadMoreRunDetail(_:))
            )
            loadMore.bezelStyle = .inline
            identify(loadMore, as: "flit.runDetail.loadMore")
            dashboardStack.addArrangedSubview(loadMore)
        }
        if pageFailure {
            let failure = label(
                FoundationCopy.text(.runDetailPageUnavailable),
                size: 12,
                weight: .semibold,
                color: .systemRed
            )
            identify(failure, as: "flit.runDetail.pageUnavailable")
            dashboardStack.addArrangedSubview(failure)
        }
        if let focusedEvidenceCursor {
            restoreRunEvidenceFocus(focusedEvidenceCursor)
        } else {
            scrollDashboardToTop()
        }
    }

    private func runCompletionSummaryView(_ summary: RunCompletionSummary) -> NSView {
        let stack = NSStackView()
        identify(stack, as: "flit.runDetail.completionSummary")
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 3
        stack.addArrangedSubview(
            label(
                FoundationCopy.text(.runDetailCompletionSummary),
                size: 13,
                weight: .semibold
            )
        )
        let facts = [
            FoundationCopy.format(.runDetailSummaryResult, summary.result),
            FoundationCopy.format(
                .runDetailSummaryProjectProvider,
                summary.projectDisplayName,
                summary.provider.rawValue
            ),
            summary.startedAt.map {
                FoundationCopy.format(.runDetailSummaryTime, $0, summary.endedAt)
            } ?? FoundationCopy.format(
                .runDetailSummaryStartUnavailable,
                summary.endedAt
            ),
            dashboardChangesCopy(summary.changes),
            FoundationCopy.text(.runDetailSummaryBranchUnavailable),
            FoundationCopy.text(.runDetailSummaryValidationUnavailable),
            FoundationCopy.text(.runDetailSummaryOpenIssuesUnavailable),
            FoundationCopy.text(.runDetailSummaryEvidenceUnavailable),
        ]
        facts.forEach {
            stack.addArrangedSubview(
                label($0, size: 11, weight: .regular, color: .secondaryLabelColor)
            )
        }
        return stack
    }

    private func runDetailGroup(
        _ group: RunActivityGroup,
        runId: String,
        hasUnloadedTail: Bool
    ) -> NSView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 2
        guard let first = group.events.first, let last = group.events.last else {
            preconditionFailure("Run Activity group must contain evidence")
        }
        identify(stack, as: "flit.runDetail.group.\(first.cursor).\(last.cursor)")
        let header = label(
            FoundationCopy.format(
                hasUnloadedTail ? .runDetailGroupLoadedThrough : .runDetailGroup,
                group.startedAt,
                group.endedAt,
                runDetailCategoryTitle(group.category),
                group.events.count
            ),
            size: 11,
            weight: .semibold,
            color: .secondaryLabelColor
        )
        identify(header, as: "flit.runDetail.groupHeader.\(first.cursor).\(last.cursor)")
        stack.addArrangedSubview(header)
        group.events.forEach {
            stack.addArrangedSubview(runDetailEventView($0, runId: runId))
        }
        return stack
    }

    private func runDetailEventView(_ event: RunActivityRow, runId: String) -> NSView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 2
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
        stack.addArrangedSubview(row)

        let toggle = RunEvidenceButton(
            runId: runId,
            eventId: event.eventId,
            cursor: event.cursor
        )
        toggle.title = FoundationCopy.text(
            expandedRunEvidenceIds.contains(event.eventId)
                ? .runDetailHideEvidence
                : .runDetailShowEvidence
        )
        toggle.bezelStyle = .inline
        toggle.target = self
        toggle.action = #selector(toggleRunEvidence(_:))
        identify(toggle, as: "flit.runDetail.evidenceToggle.\(event.cursor)")
        runEvidenceButtonsByCursor[event.cursor] = toggle
        stack.addArrangedSubview(toggle)

        if expandedRunEvidenceIds.contains(event.eventId) {
            let evidence = label(
                FoundationCopy.format(
                    .runDetailEvidence,
                    event.eventId,
                    event.eventType,
                    runDetailCategoryTitle(event.category),
                    event.sourceKind.rawValue,
                    Int(event.confidence * 100),
                    event.observedAt
                ),
                size: 11,
                weight: .regular,
                color: .secondaryLabelColor
            )
            evidence.maximumNumberOfLines = 2
            identify(evidence, as: "flit.runDetail.evidence.\(event.cursor)")
            stack.addArrangedSubview(evidence)
            let rawUnavailable = label(
                FoundationCopy.text(.runDetailRawPayloadUnavailable),
                size: 11,
                weight: .regular,
                color: .secondaryLabelColor
            )
            identify(
                rawUnavailable,
                as: "flit.runDetail.rawPayloadUnavailable.\(event.cursor)"
            )
            stack.addArrangedSubview(rawUnavailable)
        }
        return stack
    }

    private func runDetailFilterControl() -> NSView {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 8

        let filterLabel = label(
            FoundationCopy.text(.runDetailFilter),
            size: 12,
            weight: .regular,
            color: .secondaryLabelColor
        )
        identify(filterLabel, as: "flit.runDetail.filterLabel")
        stack.addArrangedSubview(filterLabel)

        let filter = NSPopUpButton()
        filter.addItems(withTitles: RunActivityFilter.allCases.map(runDetailFilterTitle))
        filter.selectItem(
            at: RunActivityFilter.allCases.firstIndex(of: activeRunDetailFilter) ?? 0
        )
        filter.target = self
        filter.action = #selector(changeRunDetailFilter(_:))
        filter.setAccessibilityLabel(FoundationCopy.text(.runDetailFilter))
        identify(filter, as: "flit.runDetail.filter")
        stack.addArrangedSubview(filter)
        return stack
    }

    private func runDetailFilterTitle(_ filter: RunActivityFilter) -> String {
        let key: FoundationCopyKey
        switch filter {
        case .all: key = .runDetailFilterAll
        case .activity: key = .runDetailFilterActivity
        case .command: key = .runDetailFilterCommand
        case .file: key = .runDetailFilterFile
        case .test: key = .runDetailFilterTest
        case .attention: key = .runDetailFilterAttention
        case .lifecycle: key = .runDetailFilterLifecycle
        }
        return FoundationCopy.text(key)
    }

    private func runDetailCategoryTitle(_ category: FlitRunEvidenceCategory) -> String {
        let filter: RunActivityFilter
        switch category {
        case .activity: filter = .activity
        case .command: filter = .command
        case .file: filter = .file
        case .test: filter = .test
        case .attention: filter = .attention
        case .lifecycle: filter = .lifecycle
        case .unknown: return FoundationCopy.text(.runDetailEvidenceUnknown)
        }
        return runDetailFilterTitle(filter)
    }

    private func runDetailFilterIdentifier(_ filter: RunActivityFilter) -> String {
        switch filter {
        case .all: "all"
        case .activity: "activity"
        case .command: "command"
        case .file: "file"
        case .test: "test"
        case .attention: "attention"
        case .lifecycle: "lifecycle"
        }
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
        runEvidenceButtonsByCursor.removeAll(keepingCapacity: true)
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

    private func restoreRunEvidenceFocus(_ cursor: UInt64) {
        guard let button = runEvidenceButtonsByCursor[cursor] else {
            scrollDashboardToTop()
            return
        }
        dashboardScroll?.documentView?.layoutSubtreeIfNeeded()
        button.scrollToVisible(button.bounds)
        button.window?.makeFirstResponder(button)
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
