import AppKit
import SwiftUI

/// Design tokens for the operator console — the committed dark instrument
/// world (DESIGN.md, the visual constitution). Dark only: the app never
/// reads system appearance and every token below is exactly one value.
/// Structure is drawn with crisp 1px borders, never shadows; the single
/// warm orange accent is budgeted (DESIGN.md §6); machine truth is always
/// monospace. No view invents its own colors.
enum Theme {
    // MARK: Surfaces — the 4-step elevation ladder. Each step is a material
    // change drawn by a border, never a shadow.

    /// Sidebar and popover list background — one step darker than content.
    static let sidebarBg = rgb(0x12, 0x11, 0x10)
    /// Window/content background, sheet background.
    static let windowBg = rgb(0x15, 0x14, 0x12)
    /// Cards, rows-at-rest surfaces, tab strips, drawer chrome.
    static let panel = rgb(0x1D, 0x1C, 0x1A)
    /// Hovered interactive rows/cards.
    static let panelHover = rgb(0x21, 0x20, 0x1D)
    /// Inputs, text editors, code/command wells, selected rows.
    static let well = rgb(0x24, 0x22, 0x20)
    /// THE structural line: 1px component borders and every seam.
    static let border = rgb(0x32, 0x30, 0x2C)
    /// Hovered/selected component border.
    static let borderHover = rgb(0x3E, 0x3B, 0x36)
    /// Pressed standard-button fill (DESIGN.md §5).
    static let pressedFill = rgb(0x17, 0x16, 0x14)

    // MARK: Text

    /// Primary UI text and all monospace machine truth.
    static let textPrimary = rgb(0xEC, 0xEA, 0xE6)
    /// Secondary text, metadata, sidebar group headers.
    static let textSecondary = rgb(0x9B, 0x96, 0x8E)
    /// Muted: placeholders, timestamps, section labels, disabled text.
    /// ~3:1 on `panel` — never the sole carrier of essential meaning.
    static let textTertiary = rgb(0x6B, 0x67, 0x5F)

    // MARK: Accent — exactly one. Permitted uses ONLY: the one primary
    // action per surface, links/inline text-buttons, live-run markers and
    // the needs-you count badge, keyboard focus rings, the brand mark.

    static let accent = rgb(0xE2, 0x70, 0x3A)
    static let accentHover = rgb(0xE8, 0x80, 0x4F)
    static let accentDown = rgb(0xD0, 0x66, 0x2F)
    /// Label ink on the accent primary fill (the window ground, not white).
    static let onAccent = rgb(0x15, 0x14, 0x12)

    // MARK: Semantics — states with fixed meanings, never decoration.

    /// Verified proof, passed checks, healthy heartbeat, service running.
    static let success = rgb(0x7B, 0xAE, 0x7F)
    /// Degradation: stale heartbeat (confirmed), tail trouble, judge-stopped
    /// review states, non-launchable previews.
    static let warn = rgb(0xD9, 0xA0, 0x48)
    /// Failure marks, borders, ≥ semibold text.
    static let danger = rgb(0xC2, 0x5B, 0x4E)
    /// Small/regular-weight red text (4.5:1-safe on all surfaces).
    static let dangerText = rgb(0xCE, 0x6A, 0x5D)
    /// Fill for the destructive confirm button only, with textPrimary label.
    static let dangerFill = rgb(0xA8, 0x45, 0x3A)

    // MARK: Overlay layers (Command-K, menus only)

    static let scrim = Color.black.opacity(0.55)
    /// Overlays only: black 25% / radius 24 / y 8 beneath their 1px border.
    static let overlayShadow = Color.black.opacity(0.25)

    // MARK: Type — fixed scale (DESIGN.md §3). System stack only: SF Pro
    // for UI, SF Mono for machine truth. No display faces anywhere.

    /// 20 semibold — empty-state and welcome headlines only.
    static let display = Font.system(size: 20, weight: .semibold)
    /// 17 semibold — sheet titles, the run-detail goal line.
    static let title = Font.system(size: 17, weight: .semibold)
    /// 15 semibold — overview card titles.
    static let heading = Font.system(size: 15, weight: .semibold)
    /// 13 regular — default UI text.
    static let base = Font.system(size: 13)
    /// 13 medium — rows, buttons, form labels.
    static let baseMedium = Font.system(size: 13, weight: .medium)
    /// 11 regular — secondary/meta lines.
    static let small = Font.system(size: 11)
    /// 10 regular — timestamps, counters, fine print.
    static let caption = Font.system(size: 10)
    /// 12 mono — command wells, primary technical lines.
    static let monoL = Font.system(size: 12, design: .monospaced)
    /// 11 mono — ids, paths, branch names inline.
    static let monoM = Font.system(size: 11, design: .monospaced)
    /// 10 mono — dense evidence rows, ledger lines, diff bodies.
    static let monoS = Font.system(size: 10, design: .monospaced)

    static func body(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }

    static func mono(_ size: CGFloat) -> Font {
        .system(size: size, design: .monospaced)
    }

    // MARK: Metrics — radii 4 (chips) / 6 (inputs, buttons, wells) /
    // 8 (cards, sheets' inner panels, tabs); 8 is the maximum.

    static let chipRadius: CGFloat = 4
    static let controlRadius: CGFloat = 6
    static let cardRadius: CGFloat = 8
    static let queueWidth: CGFloat = 860

    // MARK: Section titles

    /// THE section header (DESIGN.md §5): 10.5 bold, +0.8pt tracking,
    /// UPPERCASE, textTertiary. Sidebar group headers and any header the
    /// eye must scan-first pass textSecondary. One style, one place — no
    /// per-view sub-scales.
    static func sectionTitle(_ text: String, color: Color = textTertiary) -> some View {
        Text(text.uppercased())
            .font(.system(size: 10.5, weight: .bold))
            .kerning(0.8)
            .foregroundStyle(color)
    }

    // MARK: Provider marks — per-agent identity colors desaturated into
    // this world; unknown ids pick deterministically from
    // {accent, success, warn} by scalar sum (String.hashValue is
    // process-seeded and would repaint marks every launch).

    static func providerColor(_ provider: String) -> Color {
        switch provider.lowercased() {
        case "claude", "anthropic", "claude-code":
            return rgb(0xC8, 0x78, 0x50)
        case "codex", "openai":
            return rgb(0x5E, 0x9B, 0x8F)
        case "gemini", "google":
            return rgb(0x71, 0x89, 0xC9)
        case "opencode":
            return rgb(0x90, 0x78, 0xB8)
        default:
            let palette = [accent, success, warn]
            let sum = provider.unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
            return palette[sum % palette.count]
        }
    }

    private static func rgb(_ r: Int, _ g: Int, _ b: Int) -> Color {
        Color(red: Double(r) / 255, green: Double(g) / 255, blue: Double(b) / 255)
    }
}

/// Provider badge iconography (DESIGN.md §2): a 12–16px rounded-rect tile
/// (radius 4), fill = mark color at 14%, glyph = mark color. The glyph is
/// the route's initial — brand logos are not SF Symbols and an invented
/// glyph would be a guess. Decorative identity only: the agent's name stays
/// printed beside it wherever facts are listed, so the mark is hidden from
/// accessibility.
struct ProviderIcon: View {
    let provider: String
    var size: CGFloat = 16

    var body: some View {
        RoundedRectangle(cornerRadius: 4, style: .continuous)
            .fill(color.opacity(0.14))
            .frame(width: size, height: size)
            .overlay(
                Text(initial)
                    .font(.system(size: size * 0.6, weight: .bold))
                    .foregroundStyle(color)
            )
            .accessibilityHidden(true)
    }

    private var initial: String {
        provider.first.map { String($0).uppercased() } ?? "?"
    }

    private var color: Color {
        Theme.providerColor(provider)
    }
}

/// Card chrome shared by rows-as-cards and sheet panels: panel fill, 1px
/// border, radius 8. No shadow — borders draw the structure (DESIGN.md §4).
struct CardBackground: ViewModifier {
    var hovering = false

    func body(content: Content) -> some View {
        content
            .background(hovering ? Theme.panelHover : Theme.panel)
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                    .strokeBorder(hovering ? Theme.borderHover : Theme.border, lineWidth: 1)
            )
    }
}

extension View {
    func cardChrome(hovering: Bool = false) -> some View {
        modifier(CardBackground(hovering: hovering))
    }
}

/// Input chrome (DESIGN.md §5): well fill, 1px border, radius 6; keyboard
/// focus swaps the stroke to accent — no system glow. `stroke` overrides
/// for semantic strokes (the typed-amount confirm field).
struct InputChrome: ViewModifier {
    var focused = false
    var stroke: Color?

    func body(content: Content) -> some View {
        content
            .background(Theme.well,
                        in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                .strokeBorder(stroke ?? (focused ? Theme.accent : Theme.border), lineWidth: 1))
    }
}

extension View {
    func inputChrome(focused: Bool = false, stroke: Color? = nil) -> some View {
        modifier(InputChrome(focused: focused, stroke: stroke))
    }
}

/// The app-wide press response (DESIGN.md §5): weight, not shrinking —
/// opacity 0.85 while pressed, 120ms, no scale bounce.
struct PressButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

extension ButtonStyle where Self == PressButtonStyle {
    static var tactile: PressButtonStyle { PressButtonStyle() }
}

/// Flat chrome buttons (DESIGN.md §5): height 28 (compact 24 in dense
/// rows), radius 6, horizontal padding 12, 1px borders. Hierarchy comes
/// from fill and stroke, never shadow or scale.
struct ThemeButtonStyle: ButtonStyle {
    enum Kind {
        /// panel fill, border stroke, textPrimary label.
        case standard
        /// At most ONE per surface: accent fill, onAccent semibold label.
        case primary
        /// The final confirm inside a destructive sheet only.
        case dangerConfirm
        /// Opens a destructive flow: standard with danger stroke at 55%
        /// and dangerText label.
        case quietDanger
    }

    var kind: Kind = .standard
    var compact = false

    func makeBody(configuration: Configuration) -> some View {
        ThemeButtonBody(kind: kind, compact: compact, configuration: configuration)
    }

    private struct ThemeButtonBody: View {
        let kind: Kind
        let compact: Bool
        let configuration: Configuration
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            configuration.label
                .font(.system(size: compact ? 12.5 : 13, weight: labelWeight))
                .foregroundStyle(labelColor)
                .lineLimit(1)
                .padding(.horizontal, 12)
                .frame(height: compact ? 24 : 28)
                .background(fill,
                            in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                    .strokeBorder(stroke, lineWidth: 1))
                .contentShape(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .opacity(configuration.isPressed ? 0.85 : 1)
                .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
                .onHover { hovering = $0 }
        }

        private var labelWeight: Font.Weight {
            switch kind {
            case .standard, .quietDanger: return .medium
            case .primary, .dangerConfirm: return .semibold
            }
        }

        private var labelColor: Color {
            guard isEnabled else { return Theme.textTertiary }
            switch kind {
            case .standard: return Theme.textPrimary
            case .primary: return Theme.onAccent
            case .dangerConfirm: return Theme.textPrimary
            case .quietDanger: return Theme.dangerText
            }
        }

        private var fill: Color {
            guard isEnabled else { return Theme.panel }
            switch kind {
            case .standard, .quietDanger:
                if configuration.isPressed { return Theme.pressedFill }
                return hovering ? Theme.well : Theme.panel
            case .primary:
                if configuration.isPressed { return Theme.accentDown }
                return hovering ? Theme.accentHover : Theme.accent
            case .dangerConfirm:
                return Theme.dangerFill
            }
        }

        private var stroke: Color {
            guard isEnabled else { return Theme.border }
            switch kind {
            case .standard:
                return hovering ? Theme.borderHover : Theme.border
            case .quietDanger:
                return Theme.danger.opacity(0.55)
            case .primary, .dangerConfirm:
                return .clear
            }
        }
    }
}

extension ButtonStyle where Self == ThemeButtonStyle {
    static var themeStandard: ThemeButtonStyle { ThemeButtonStyle(kind: .standard) }
    static var themePrimary: ThemeButtonStyle { ThemeButtonStyle(kind: .primary) }
    static var themeDangerConfirm: ThemeButtonStyle { ThemeButtonStyle(kind: .dangerConfirm) }
    static var themeQuietDanger: ThemeButtonStyle { ThemeButtonStyle(kind: .quietDanger) }

    static func themeStandard(compact: Bool) -> ThemeButtonStyle {
        ThemeButtonStyle(kind: .standard, compact: compact)
    }
}

/// Text-button / link (DESIGN.md §5): accent text, no chrome, underline on
/// hover. Inline fixes, "show output", disclosure toggles.
struct ThemeTextButtonStyle: ButtonStyle {
    var size: CGFloat = 12

    func makeBody(configuration: Configuration) -> some View {
        TextButtonBody(size: size, configuration: configuration)
    }

    private struct TextButtonBody: View {
        let size: CGFloat
        let configuration: Configuration
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            configuration.label
                .font(.system(size: size, weight: .medium))
                .foregroundStyle(isEnabled ? Theme.accent : Theme.textTertiary)
                .underline(hovering && isEnabled)
                .opacity(configuration.isPressed ? 0.85 : 1)
                .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
                .onHover { hovering = $0 }
        }
    }
}

extension ButtonStyle where Self == ThemeTextButtonStyle {
    static var themeText: ThemeTextButtonStyle { ThemeTextButtonStyle() }

    static func themeText(size: CGFloat) -> ThemeTextButtonStyle {
        ThemeTextButtonStyle(size: size)
    }
}

/// The one status atom (DESIGN.md §5): height 18, radius 4, fill = color at
/// 10%, stroke at 45%, text = the color. `strong` (16% / 70%) is reserved
/// for VERIFIED / PROOF INVALID / CHANGED SINCE APPROVAL-class facts. No
/// filled-solid chips — the border world carries state by line and hue.
/// Red chips pass `textColor: Theme.dangerText` so small red text stays
/// contrast-safe while the border keeps the deeper `danger` hue.
struct StatusChip: View {
    let text: String
    var color: Color = Theme.textSecondary
    var strong = false
    var textColor: Color?

    var body: some View {
        Text(text)
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundStyle(textColor ?? color)
            .padding(.horizontal, 6)
            .frame(height: 18)
            .background(color.opacity(strong ? 0.16 : 0.10),
                        in: RoundedRectangle(cornerRadius: Theme.chipRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.chipRadius, style: .continuous)
                .strokeBorder(color.opacity(strong ? 0.70 : 0.45), lineWidth: 1))
            .lineLimit(1)
    }
}

/// The only pill in the app (DESIGN.md §5): the needs-you count badge.
/// 16px, accent fill, onAccent 10 bold text. Never for plain totals.
struct CountBadge: View {
    let count: Int

    var body: some View {
        Text("\(count)")
            .font(.system(size: 10, weight: .bold))
            .monospacedDigit()
            .foregroundStyle(Theme.onAccent)
            .padding(.horizontal, 5)
            .frame(minWidth: 16)
            .frame(height: 16)
            .background(Theme.accent, in: Capsule())
    }
}

/// A 6px state dot (DESIGN.md §5): accent = live, success = verified
/// awaiting approval, warn = stopped/paused for you, danger = failed,
/// textTertiary = queued/waiting/finished-quiet. Never appears without a
/// word or a labeled row.
struct StateDot: View {
    let color: Color

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 6, height: 6)
            .accessibilityHidden(true)
    }
}

/// A text tab in a panel strip (DESIGN.md §5): 12.5 medium; active =
/// textPrimary on well (radius 6, 1px border), inactive = textSecondary,
/// hover = panelHover. Counts ride in the title as plain text.
struct TabButton: View {
    let title: String
    let active: Bool
    let action: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(active ? Theme.textPrimary : Theme.textSecondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 4)
                .background(
                    active ? Theme.well : (hovering ? Theme.panelHover : .clear),
                    in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                    .strokeBorder(active ? Theme.border : .clear, lineWidth: 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .onHover { hovering = $0 }
    }
}
