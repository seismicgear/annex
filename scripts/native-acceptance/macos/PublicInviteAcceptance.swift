import XCTest
import AppKit
import CryptoKit

// This recipient reaches an independently running native Host over the actual
// public service. The test never provisions a destination, founder or session.
final class PublicInviteAcceptance: XCTestCase {
    let safari = XCUIApplication(bundleIdentifier: "com.apple.Safari")
    let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
    var phase = "before-public-invite"
    var events: [[String: Any]] = []
    var evidence: URL { URL(fileURLWithPath: ProcessInfo.processInfo.environment["ANNEX_EVIDENCE_DIR"]!) }

    override func setUpWithError() throws { continueAfterFailure = false }
    override func tearDownWithError() throws {
        if safari.state != .notRunning { capture(safari, "final-safari-" + phase) }
        if annex.state != .notRunning { capture(annex, "final-annex-" + phase) }
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: events, options: [.prettyPrinted, .sortedKeys]), uniformTypeIdentifier: "public.json")
        attachment.name = "annex-public-invite-events"; attachment.lifetime = .keepAlways; add(attachment)
    }

    func capture(_ app: XCUIApplication, _ name: String) {
        phase = name
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = name; screenshot.lifetime = .keepAlways; add(screenshot)
        let tree = XCTAttachment(string: app.debugDescription)
        tree.name = name + "-accessibility"; tree.lifetime = .keepAlways; add(tree)
        events.append(["phase": name, "at": ISO8601DateFormatter().string(from: Date()),
                       "application": app === safari ? "Safari" : "Annex"])
    }

    func click(_ app: XCUIApplication, _ title: String, timeout: TimeInterval = 60) {
        let button = app.buttons[title].firstMatch
        if !button.waitForExistence(timeout: timeout) { capture(app, "missing-" + title) }
        XCTAssertTrue(button.exists, "Missing actual UI button: " + title)
        XCTAssertTrue(button.isHittable)
        button.click()
    }

    func textExists(_ app: XCUIApplication, _ text: String, timeout: TimeInterval = 30) -> Bool {
        // Read one immutable accessibility snapshot per attempt. WebKit text
        // can be in either label or value; headings also contain numeric values.
        // Index-bound live elements race removal of the connecting screen.
        func contains(_ node: XCUIElementSnapshot) -> Bool {
            if node.elementType == .staticText,
               node.label.contains(text) || (node.value as? String)?.contains(text) == true {
                return true
            }
            return node.children.contains(where: contains)
        }
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if let snapshot = try? app.windows.firstMatch.snapshot(), contains(snapshot) { return true }
            Thread.sleep(forTimeInterval: 0.5)
        } while Date() < deadline
        return false
    }

    func runningAnnex() -> NSRunningApplication? {
        let apps = NSRunningApplication.runningApplications(withBundleIdentifier: "com.annex.desktop")
        XCTAssertLessThanOrEqual(apps.count, 1)
        return apps.first
    }

    func staticText(_ node: XCUIElementSnapshot, contains text: String) -> Bool {
        if node.elementType == .staticText,
           node.label.contains(text) || (node.value as? String)?.contains(text) == true { return true }
        return node.children.contains { staticText($0, contains: text) }
    }

    func messageHistory(_ node: XCUIElementSnapshot) -> XCUIElementSnapshot? {
        if node.label == "Message history" { return node }
        for child in node.children { if let history = messageHistory(child) { return history } }
        return nil
    }

    func waitForAnnex(timeout: TimeInterval, condition: (XCUIElementSnapshot) -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if let snapshot = try? annex.windows.firstMatch.snapshot(), condition(snapshot) { return true }
            Thread.sleep(forTimeInterval: 0.5)
        } while Date() < deadline
        return false
    }

    func waitForCompletedInvite(_ slug: String) -> Bool {
        waitForAnnex(timeout: 180) { snapshot in
            self.staticText(snapshot, contains: slug)
                && !self.staticText(snapshot, contains: "Invite received for")
                && !self.staticText(snapshot, contains: "Connecting to the invited server")
                && !self.annex.buttons["Join invite server"].firstMatch.exists
                && !self.annex.buttons["Joining invite server…"].firstMatch.exists
        }
    }

    func openFromPublicPage() {
        click(safari, "Open in Annex")
        for _ in 0..<15 {
            if annex.state == .runningForeground { break }
            let dialog = safari.dialogs["AXSafariModalDialog"].firstMatch
            if dialog.exists {
                let message = dialog.textViews["SFDialogViewMessageText"].firstMatch
                XCTAssertTrue((message.value as? String ?? "").contains("Annex"))
                let allow = dialog.descendants(matching: .any).matching(
                    NSPredicate(format: "label == %@ OR title == %@", "Allow", "Allow")).firstMatch
                XCTAssertTrue(allow.waitForExistence(timeout: 10))
                XCTAssertTrue(allow.isHittable)
                capture(safari, "ordinary-open-annex-confirmation")
                allow.click()
                break
            }
            Thread.sleep(forTimeInterval: 1)
        }
        let foreground = NSPredicate { _, _ in self.annex.state == .runningForeground }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: foreground, object: nil)], timeout: 60), .completed,
                       "The public invitation must open Annex through the operating system")
        XCTAssertEqual(runningAnnex()?.bundleURL?.path, "/Applications/Annex.app")
    }

    func testPublicInviteToIndependentNativeHost() throws {
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: evidence.appendingPathComponent("fixture.json"))) as? [String: Any])
        let invite = try XCTUnwrap(fixture["invite"] as? String)
        let origin = try XCTUnwrap(fixture["origin"] as? String)
        let slug = try XCTUnwrap(fixture["slug"] as? String)
        let label = try XCTUnwrap(fixture["label"] as? String)
        let message = "ANNEX_PUBLIC_MAC_RECIPIENT_" + UUID().uuidString
        XCTAssertNil(runningAnnex(), "The installed recipient must be cold before the public link opens it")
        safari.launch()
        XCTAssertTrue(safari.windows.firstMatch.waitForExistence(timeout: 30))
        safari.typeKey("l", modifierFlags: .command)
        safari.typeText(invite)
        safari.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(safari.buttons["Open in Annex"].firstMatch.waitForExistence(timeout: 60))
        XCTAssertTrue(textExists(safari, label))
        XCTAssertTrue(textExists(safari, origin))
        capture(safari, "live-monolithannex-invitation")
        openFromPublicPage()
        let recipientPID = try XCTUnwrap(runningAnnex()?.processIdentifier)
        capture(annex, "installed-recipient-cold-opened-by-public-invite")
        click(annex, "Create New Identity")
        XCTAssertTrue(annex.buttons["Join invite server"].firstMatch.waitForExistence(timeout: 120))
        XCTAssertTrue(textExists(annex, origin))
        capture(annex, "exact-public-destination-before-join")
        click(annex, "Join invite server")
        XCTAssertTrue(waitForCompletedInvite(slug), "The invite must complete, not leave a matching server behind a joining banner")
        XCTAssertTrue(textExists(annex, label))
        capture(annex, "native-recipient-joined-independent-public-host")

        click(annex, "Identity")
        XCTAssertTrue(textExists(annex, slug))
        XCTAssertFalse(annex.staticTexts["Server founder"].firstMatch.exists,
                       "An invited recipient must not inherit the host's founder authority")
        capture(annex, "recipient-profile-has-no-founder-authority")
        annex.typeKey(.escape, modifierFlags: [])
        let general = annex.buttons.matching(NSPredicate(format: "label CONTAINS %@ OR title CONTAINS %@", "General", "General")).firstMatch
        XCTAssertTrue(general.waitForExistence(timeout: 60)); general.click()
        let join = annex.buttons["Join channel"].firstMatch
        if join.waitForExistence(timeout: 3) { join.click() }
        let composer = annex.textViews.firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: 60))
        XCTAssertTrue(waitForAnnex(timeout: 120) { snapshot in
            self.messageHistory(snapshot) != nil
                && !self.staticText(snapshot, contains: "Checking channel encryption")
                && !self.staticText(snapshot, contains: "Loading channel history")
        }, "The actual channel must finish loading before sending")
        composer.click(); composer.typeText(message)
        click(annex, "Send")
        XCTAssertTrue(waitForAnnex(timeout: 120) { snapshot in
            guard let history = self.messageHistory(snapshot) else { return false }
            return self.staticText(history, contains: message)
                && !self.staticText(history, contains: "sending...")
                && !self.staticText(history, contains: "failed")
                && (composer.value as? String ?? "").isEmpty
        }, "Only a message acknowledged in history with an empty composer counts; a draft or pending message never does")
        capture(annex, "native-recipient-sent-correlation-message")

        safari.activate()
        XCTAssertTrue(safari.buttons["Open in Annex"].firstMatch.waitForExistence(timeout: 30))
        openFromPublicPage()
        XCTAssertEqual(runningAnnex()?.processIdentifier, recipientPID)
        XCTAssertTrue(annex.buttons["Join invite server"].firstMatch.waitForExistence(timeout: 30))
        XCTAssertTrue(textExists(annex, origin))
        capture(annex, "public-invite-delivered-to-existing-native-recipient")
        click(annex, "Join invite server")
        XCTAssertTrue(waitForCompletedInvite(slug))
        XCTAssertEqual(runningAnnex()?.processIdentifier, recipientPID)
        capture(annex, "warm-invite-rejoined-the-same-server")

        // Reopen through Safari after membership was persisted. A cached
        // active server ID must not skip restoring its API/identity context.
        annex.terminate()
        XCTAssertNil(runningAnnex())
        safari.activate()
        openFromPublicPage()
        let reopenedPID = try XCTUnwrap(runningAnnex()?.processIdentifier)
        XCTAssertNotEqual(reopenedPID, recipientPID)
        click(annex, "Join invite server", timeout: 120)
        XCTAssertTrue(waitForCompletedInvite(slug))
        XCTAssertFalse(annex.buttons["Create New Identity"].firstMatch.exists)
        capture(annex, "cold-public-invite-restored-existing-membership")
        let receipt: [String: Any] = ["passed": true, "destination": origin, "server_slug": slug,
            "server_label": label, "invite_sha256": SHA256.hash(data: Data(invite.utf8)).map { String(format: "%02x", $0) }.joined(),
            "native_pid": recipientPID, "message": message, "cold_launch_by_public_link": true,
            "explicit_join": true, "recipient_is_founder": false, "warm_delivery_same_process": true,
            "warm_same_server_join_completed": true, "cold_reopen_join_completed": true,
            "reopened_native_pid": reopenedPID,
            "native_app_launched_directly_by_test": false,
            "message_acknowledged_in_history": true, "composer_empty_after_send": true,
            "independent_host_receipt_required_for_message_delivery": true]
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted, .sortedKeys]), uniformTypeIdentifier: "public.json")
        attachment.name = "annex-native-public-invite-result"; attachment.lifetime = .keepAlways; add(attachment)
    }
}
