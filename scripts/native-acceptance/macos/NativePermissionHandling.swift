import XCTest
import AppKit
import CoreGraphics
import Vision

extension XCTestCase {
    private func isExpectedLocalNetworkPrompt(_ text: String) -> Bool {
        // Match the actual permission subject, not an Annex title elsewhere in
        // the captured application window. Python is only our updater transport
        // fixture, and is permitted only by that test's explicit environment.
        let fixture = ProcessInfo.processInfo.environment["ANNEX_UPDATER_CONTROL_TOKEN"] != nil
        let subjects = fixture ? "(Annex|hosted-compute-[^\"”]+|Python)" : "(Annex|hosted-compute-[^\"”]+)"
        let pattern = "Allow [\"“]" + subjects + "[\"”] to find devices on local networks\\?"
        return text.range(of: pattern, options: .regularExpression) != nil
    }

    // macOS 15's UserNotificationCenter may render a permission window without
    // exposing its controls to XCTest. Read that actual window's pixels and
    // click only its recognized Allow button; never use guessed coordinates,
    // change TCC databases, or suppress the operating system's consent UI.
    private func acceptRenderedLocalNetworkPrompt(_ windows: [[String: Any]]) -> Bool {
        for window in windows {
            guard let pid = (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,
                  let bundle = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier,
                  ["com.apple.UserNotificationCenter", "com.annex.desktop"].contains(bundle),
                  let dictionary = window[kCGWindowBounds as String] as? NSDictionary,
                  let frame = CGRect(dictionaryRepresentation: dictionary as CFDictionary), frame.width > 100, frame.height > 100 else { continue }
            let screenshot = XCUIScreen.main.screenshot()
            let image = screenshot.image
            guard let full = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { continue }
            let scale = CGFloat(full.width) / image.size.width
            let pixelFrame = CGRect(x: frame.minX * scale, y: frame.minY * scale,
                                    width: frame.width * scale, height: frame.height * scale)
            guard let crop = full.cropping(to: pixelFrame) else { continue }
            let request = VNRecognizeTextRequest()
            request.recognitionLevel = .accurate
            request.recognitionLanguages = ["en-US"]
            do { try VNImageRequestHandler(cgImage: crop).perform([request]) } catch { continue }
            let rows = (request.results ?? []).compactMap { row -> (String, CGRect)? in
                guard let text = row.topCandidates(1).first, text.confidence >= 0.8 else { return nil }
                return (text.string, row.boundingBox)
            }
            let text = rows.map { $0.0 }.joined(separator: " ")
            guard isExpectedLocalNetworkPrompt(text) else { continue }
            let allows = rows.filter { $0.0 == "Allow" }
            guard allows.count == 1 else { continue }
            let box = allows[0].1
            let point = CGPoint(x: frame.minX + box.midX * frame.width,
                                y: frame.minY + (1 - box.midY) * frame.height)
            guard frame.contains(point) else { return false }
            let attachment = XCTAttachment(screenshot: screenshot)
            attachment.name = "observed-local-network-permission-before-allow"
            attachment.lifetime = .keepAlways; add(attachment)
            let observation = XCTAttachment(string: "Window: \(frame)\nRecognized: \(text)\nAllow: \(point)")
            observation.name = "observed-local-network-permission-control"
            observation.lifetime = .keepAlways; add(observation)
            // XCTest already owns authorized UI automation. Raw CGEvent posts
            // instead request a second Accessibility permission and can silently
            // fail behind that prompt. Use XCTest at the observed screen point.
            let anchor = XCUIApplication(bundleIdentifier: "com.annex.desktop").windows.firstMatch
            guard anchor.exists else { return false }
            let origin = anchor.frame.origin
            anchor.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
                .withOffset(CGVector(dx: point.x - origin.x, dy: point.y - origin.y)).click()
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
            return true
        }
        return false
    }

    @discardableResult
    private func acceptAnnexLocalNetworkPrompt(_ element: XCUIElement) -> Bool {
        let description = element.debugDescription
        guard isExpectedLocalNetworkPrompt(description.replacingOccurrences(of: "\n", with: " ")) else { return false }
        let buttons = element.descendants(matching: .button).matching(
            NSPredicate(format: "label == %@ OR title == %@", "Allow", "Allow"))
        guard let allow = buttons.allElementsBoundByIndex.first(where: { $0.isEnabled && $0.isHittable }) else { return false }
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "annex-local-network-permission"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let tree = XCTAttachment(string: description)
        tree.name = "annex-local-network-permission-accessibility"
        tree.lifetime = .keepAlways
        add(tree)
        allow.click()
        return true
    }

    @discardableResult
    func acceptVisibleAnnexLocalNetworkPermission() -> Bool {
        // The macOS 15 prompt can belong to a system application rather than
        // either test application, so XCTest's interruption monitor may not fire.
        // Discover its real running owner; never launch or modify system apps.
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        let owners = Set(windows.compactMap { ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value })
        let bundles = Set(owners.compactMap { NSRunningApplication(processIdentifier: $0)?.bundleIdentifier })
        let inventory = XCTAttachment(string: windows.map { window in
            let pid = (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value ?? 0
            let bundle = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier ?? "unknown"
            return "\(bundle) pid=\(pid) bounds=\(String(describing: window[kCGWindowBounds as String]))"
        }.joined(separator: "\n"))
        inventory.name = "visible-permission-window-owners"
        inventory.lifetime = .keepAlways
        add(inventory)
        // Annex's own process also owns this system consent window on macOS 15.
        // Prefer the visible frontmost window; an occluded prompt may have an
        // incomplete OCR result and must not hide the next observed window.
        if acceptRenderedLocalNetworkPrompt(windows) { return true }
        for bundle in bundles.sorted() where bundle.hasPrefix("com.apple.") || bundle == "com.annex.desktop" {
            let owner = XCUIApplication(bundleIdentifier: bundle)
            // Some system consent surfaces are AX dialogs, not AX windows.
            // Inspect the observed owner root so both shapes are included.
            if acceptAnnexLocalNetworkPrompt(owner) { return true }
        }
        return false
    }

    func monitorAnnexLocalNetworkPermission() {
        // macOS 15 can show this ordinary first-launch consent after Host has
        // already opened. Handle only Annex's local-network request on these
        // disposable acceptance machines; never dismiss unrelated dialogs.
        addUIInterruptionMonitor(withDescription: "Annex local-network permission") { alert in
            self.acceptAnnexLocalNetworkPrompt(alert)
        }
    }
}
