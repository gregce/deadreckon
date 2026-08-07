import AppKit
import SwiftUI

/// Design tokens for the operator console, mirroring the specstory-mac
/// Granola-style shell: warm paper surfaces, hairline borders, serif display
/// headers, quiet grays. Light-first with dark support. Every new view uses
/// these tokens; no view invents its own colors.
enum Theme {
    // MARK: Surfaces

    static let paper = dynamicColor(light: NSColor(red: 0.984, green: 0.980, blue: 0.968, alpha: 1),
                                    dark: NSColor(red: 0.113, green: 0.111, blue: 0.105, alpha: 1))
    static let card = dynamicColor(light: .white,
                                   dark: NSColor(red: 0.157, green: 0.155, blue: 0.148, alpha: 1))
    static let cardHover = dynamicColor(light: NSColor(red: 0.975, green: 0.970, blue: 0.958, alpha: 1),
                                        dark: NSColor(red: 0.190, green: 0.188, blue: 0.180, alpha: 1))
    static let hairline = dynamicColor(light: NSColor.black.withAlphaComponent(0.08),
                                       dark: NSColor.white.withAlphaComponent(0.10))

    // MARK: Text

    static let ink = dynamicColor(light: NSColor(red: 0.13, green: 0.12, blue: 0.11, alpha: 1),
                                  dark: NSColor(red: 0.93, green: 0.92, blue: 0.90, alpha: 1))
    static let inkSecondary = dynamicColor(light: NSColor(red: 0.42, green: 0.41, blue: 0.39, alpha: 1),
                                           dark: NSColor(red: 0.66, green: 0.65, blue: 0.63, alpha: 1))
    static let inkTertiary = dynamicColor(light: NSColor(red: 0.62, green: 0.61, blue: 0.58, alpha: 1),
                                          dark: NSColor(red: 0.48, green: 0.47, blue: 0.45, alpha: 1))

    // MARK: Accents

    static let accent = dynamicColor(light: NSColor(red: 0.20, green: 0.55, blue: 0.72, alpha: 1),
                                     dark: NSColor(red: 0.38, green: 0.70, blue: 0.85, alpha: 1))
    /// Live activity (running/verifying) and the breathing dot.
    static let live = dynamicColor(light: NSColor(red: 0.16, green: 0.45, blue: 0.94, alpha: 1),
                                   dark: NSColor(red: 0.45, green: 0.66, blue: 1.00, alpha: 1))
    /// Healthy/verified green: fresh heartbeats, VERIFIED chips, ok counts.
    static let verified = dynamicColor(light: NSColor(red: 0.23, green: 0.60, blue: 0.36, alpha: 1),
                                       dark: NSColor(red: 0.38, green: 0.75, blue: 0.50, alpha: 1))
    /// Amber warnings: stale leases, decision-needed badges, uncertain facts.
    static let warn = dynamicColor(light: NSColor(red: 0.78, green: 0.55, blue: 0.10, alpha: 1),
                                   dark: NSColor(red: 0.92, green: 0.72, blue: 0.32, alpha: 1))
    /// Filled-chip amber. `warn`'s light-mode value is tuned for TEXT on
    /// paper; as a FILL under white `onFill` text it lands near 2.9:1.
    /// Filled amber chips use this darker light-mode fill (white text
    /// clears 4.5:1); the dark-mode value matches `warn`, where the dark
    /// `onFill` ink already clears contrast on the lighter amber.
    static let warnFill = dynamicColor(light: NSColor(red: 0.60, green: 0.42, blue: 0.05, alpha: 1),
                                       dark: NSColor(red: 0.92, green: 0.72, blue: 0.32, alpha: 1))
    /// Filled-chip green, mirroring `warnFill`: `verified`'s light-mode
    /// value under white `onFill` text computes to ~3.6:1 as a FILL —
    /// below the 4.5:1 bar for filled chips, on the app's single most
    /// trust-critical signal. Filled VERIFIED chips use this darker
    /// light-mode green (~5.8:1 under white); the dark value matches
    /// `verified`, where the near-ink dark `onFill` already clears
    /// contrast on the lighter green.
    static let verifiedFill = dynamicColor(light: NSColor(red: 0.16, green: 0.45, blue: 0.26, alpha: 1),
                                           dark: NSColor(red: 0.38, green: 0.75, blue: 0.50, alpha: 1))
    /// Failure red: wrecked rows, proof-invalid chips, unavailable banners.
    static let danger = dynamicColor(light: NSColor(red: 0.78, green: 0.24, blue: 0.20, alpha: 1),
                                     dark: NSColor(red: 0.94, green: 0.45, blue: 0.40, alpha: 1))

    // MARK: Overlay layers

    /// Dimmed backdrop behind overlays (Command-K): deeper in dark mode so
    /// the floating panel still reads as a separate layer.
    static let scrim = dynamicColor(light: NSColor.black.withAlphaComponent(0.25),
                                    dark: NSColor.black.withAlphaComponent(0.45))
    /// Drop shadow for floating panels.
    static let overlayShadow = Color.black.opacity(0.25)
    /// Text and glyphs on filled chips: white over the saturated light-mode
    /// fills, near-ink over the lighter dark-mode fills (plain white fails
    /// contrast on the light dark-mode accent fills).
    static let onFill = dynamicColor(light: .white,
                                     dark: NSColor(red: 0.113, green: 0.111, blue: 0.105, alpha: 1))

    // MARK: Type

    /// Serif display face for section headers (the Granola look).
    static func display(_ size: CGFloat, weight: Font.Weight = .semibold) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    static func body(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }

    static func mono(_ size: CGFloat) -> Font {
        .system(size: size, design: .monospaced)
    }

    // MARK: Metrics

    static let cardRadius: CGFloat = 10
    static let queueWidth: CGFloat = 860

    // MARK: Section titles

    /// The one kerned-uppercase section title (10pt bold / kerning 0.6 /
    /// tertiary ink by default). Every band/section header renders through
    /// this; deliberate sub-scales (the evidence rail's and center-pane
    /// claims' 9pt bands, the drawer toggle's 9.5pt, Settings' 11pt group
    /// titles) pass size/kerning explicitly so the metric still lives in
    /// exactly one place.
    static func sectionTitle(_ text: String, size: CGFloat = 10,
                             kerning: CGFloat = 0.6) -> some View {
        Text(text)
            .font(body(size, weight: .bold))
            .kerning(kerning)
            .foregroundStyle(inkTertiary)
    }

    // MARK: Provider marks

    /// Stable per-provider color pairs for `ProviderIcon`. Known route ids
    /// get brand-adjacent dynamic pairs; anything else picks
    /// deterministically from the accent-class palette by scalar sum
    /// (String.hashValue is process-seeded and would repaint marks every
    /// launch).
    static func providerColor(_ provider: String) -> Color {
        switch provider.lowercased() {
        case "claude", "anthropic", "claude-code":
            return dynamicColor(light: NSColor(red: 0.80, green: 0.42, blue: 0.18, alpha: 1),
                                dark: NSColor(red: 0.92, green: 0.58, blue: 0.34, alpha: 1))
        case "codex", "openai":
            return dynamicColor(light: NSColor(red: 0.16, green: 0.52, blue: 0.47, alpha: 1),
                                dark: NSColor(red: 0.36, green: 0.72, blue: 0.66, alpha: 1))
        case "gemini", "google":
            return dynamicColor(light: NSColor(red: 0.26, green: 0.42, blue: 0.82, alpha: 1),
                                dark: NSColor(red: 0.50, green: 0.64, blue: 0.98, alpha: 1))
        case "opencode":
            return dynamicColor(light: NSColor(red: 0.48, green: 0.32, blue: 0.72, alpha: 1),
                                dark: NSColor(red: 0.68, green: 0.55, blue: 0.92, alpha: 1))
        default:
            let palette = [accent, verified, warn, live]
            let sum = provider.unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
            return palette[sum % palette.count]
        }
    }

    static func dynamicColor(light: NSColor, dark: NSColor) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        })
    }
}

/// Provider badge iconography (design doc section 9's copy-outright
/// exemplar pattern): a rounded-rect brand mark anchoring every surface
/// that names a provider. The glyph is the route's initial — brand logos
/// are not SF Symbols and an invented glyph would be a guess. Decorative
/// identity only: the provider word stays printed beside it wherever facts
/// are listed, so the mark is hidden from accessibility.
struct ProviderIcon: View {
    let provider: String
    var size: CGFloat = 16

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.3, style: .continuous)
            .fill(color.opacity(0.12))
            .frame(width: size, height: size)
            .overlay(
                Text(initial)
                    .font(.system(size: size * 0.6, weight: .bold, design: .rounded))
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

/// Card chrome shared by queue rows and sheets (exemplar CardBackground).
struct CardBackground: ViewModifier {
    var hovering = false

    func body(content: Content) -> some View {
        content
            .background(hovering ? Theme.cardHover : Theme.card)
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                    .strokeBorder(Theme.hairline, lineWidth: 1)
            )
            .shadow(color: .black.opacity(hovering ? 0.07 : 0.04), radius: hovering ? 6 : 3, y: 1)
    }
}

extension View {
    func cardChrome(hovering: Bool = false) -> some View {
        modifier(CardBackground(hovering: hovering))
    }
}

/// The app-wide press response (exemplar TactileButtonStyle).
struct TactileButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.96 : 1)
            .opacity(configuration.isPressed ? 0.8 : 1)
            .animation(.spring(response: 0.24, dampingFraction: 0.65), value: configuration.isPressed)
    }
}

/// Press response for large surfaces (cards, rows): weight, not shrinking.
struct TactileCardButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.985 : 1)
            .animation(.spring(response: 0.28, dampingFraction: 0.7), value: configuration.isPressed)
    }
}

extension ButtonStyle where Self == TactileButtonStyle {
    static var tactile: TactileButtonStyle { TactileButtonStyle() }
}

extension ButtonStyle where Self == TactileCardButtonStyle {
    static var tactileCard: TactileCardButtonStyle { TactileCardButtonStyle() }
}

/// The one quiet chip everywhere: counts and states, not prose (P8).
struct StatusChip: View {
    let text: String
    var color: Color = Theme.inkSecondary
    var filled = false

    var body: some View {
        Text(text)
            .font(Theme.body(10, weight: .semibold))
            .foregroundStyle(filled ? Theme.onFill : color)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(
                filled ? color : color.opacity(0.12),
                in: Capsule()
            )
            .lineLimit(1)
    }
}
