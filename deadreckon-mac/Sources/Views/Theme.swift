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

    static func dynamicColor(light: NSColor, dark: NSColor) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        })
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
