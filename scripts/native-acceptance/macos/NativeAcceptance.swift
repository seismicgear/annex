import XCTest
import AppKit
import CryptoKit

final class NativeAcceptance: XCTestCase {
    let safari = XCUIApplication(bundleIdentifier: "com.apple.Safari")
    let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
    var phase = "initial"
    var events: [[String: Any]] = []
    var evidence: URL { URL(fileURLWithPath: ProcessInfo.processInfo.environment["ANNEX_EVIDENCE_DIR"]!) }

    override func setUpWithError() throws { continueAfterFailure = false }
    override func tearDownWithError() throws {
        // XCTest assertion failures can skip Swift defer. Preserve both apps here.
        if safari.state != .notRunning { capture(safari, "final-safari-" + phase) }
        if annex.state != .notRunning { capture(annex, "final-annex-" + phase) }
        try JSONSerialization.data(withJSONObject: events, options: [.prettyPrinted, .sortedKeys])
            .write(to: evidence.appendingPathComponent("ui-events.json"))
    }

    func capture(_ app: XCUIApplication, _ name: String) {
        phase = name
        let hierarchyText = app.debugDescription
        print("ANNEX_UI_HIERARCHY \(name)\n\(hierarchyText)")
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = name; screenshot.lifetime = .keepAlways; add(screenshot)
        let hierarchy = XCTAttachment(string: hierarchyText)
        hierarchy.name = name + "-accessibility"; hierarchy.lifetime = .keepAlways; add(hierarchy)
        events.append(["phase": name, "at": ISO8601DateFormatter().string(from: Date()),
                       "application": app.label, "state": app.state.rawValue])
    }

    func click(_ app: XCUIApplication, _ title: String, timeout: TimeInterval = 60) {
        phase = "before-click-" + title
        let button = app.buttons[title].firstMatch
        if !button.waitForExistence(timeout: timeout) { capture(app, "missing-button-" + title) }
        XCTAssertTrue(button.exists, "Missing actual UI button: " + title)
        button.click()
    }

    func navigate(_ address: String) {
        safari.typeKey("l", modifierFlags: .command)
        safari.typeText(address)
        safari.typeKey(.return, modifierFlags: [])
    }

    func snapshot() -> [String: Any]? {
        guard let text = try? String(contentsOf: evidence.appendingPathComponent("database-observations.jsonl"), encoding: .utf8),
              let line = text.split(separator: "\n").last,
              let data = String(line).data(using: .utf8),
              let row = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return row
    }

    func waitForAdmissionCount(_ count: Int, timeout: TimeInterval = 90) {
        let expected = NSPredicate { _, _ in
            (self.snapshot()?["admitted_identities"] as? [[String: Any]])?.count == count
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: expected, object: nil)], timeout: timeout), .completed)
    }

    func textExists(_ app: XCUIApplication, _ text: String, timeout: TimeInterval = 30) -> Bool {
        let expected = NSPredicate { _, _ in app.debugDescription.contains(text) }
        return XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: expected, object: nil)], timeout: timeout) == .completed
    }

    func selectGeneral(_ app: XCUIApplication) {
        let button = app.buttons.matching(NSPredicate(format: "label CONTAINS %@ OR title CONTAINS %@", "General", "General")).firstMatch
        XCTAssertTrue(button.waitForExistence(timeout: 60))
        button.click()
        let join = app.buttons["Join channel"].firstMatch
        if join.waitForExistence(timeout: 3) { join.click() }
    }

    func runningAnnex() -> NSRunningApplication? {
        NSRunningApplication.runningApplications(withBundleIdentifier: "com.annex.desktop").first
    }

    func openFromPublicPage() {
        click(safari, "Open in Annex")
        // Only the ordinary Safari external-application confirmation, when shown.
        for _ in 0..<15 {
            if annex.state == .runningForeground { break }
            let allow = safari.buttons["Allow"].firstMatch
            let open = safari.buttons["Open"].firstMatch
            if (allow.exists || open.exists) && safari.debugDescription.contains("Annex") {
                capture(safari, "ordinary-open-application-prompt")
                if allow.exists { allow.click() } else { open.click() }
                break
            }
            Thread.sleep(forTimeInterval: 1)
        }
        let foreground = NSPredicate { _, _ in self.annex.state == .runningForeground }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: foreground, object: nil)], timeout: 60), .completed,
                       "The public link must launch Annex; the test never launches the recipient itself")
        XCTAssertEqual(runningAnnex()?.bundleURL?.path, "/Applications/Annex.app")
    }

    func testPublishedInviteJourney() throws {
        let fixturePath = ProcessInfo.processInfo.environment["ANNEX_NATIVE_FIXTURE"]!
        let fixture = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: fixturePath))) as! [String: Any]
        let origin = fixture["origin"] as! String
        let slug = fixture["slug"] as! String
        let label = fixture["label"] as! String
        let message = "ANNEX_MAC_NATIVE_ACCEPTANCE_" + slug + "_" + UUID().uuidString
        XCTAssertNil(runningAnnex(), "The installed recipient must not already be running")
        waitForAdmissionCount(0)

        safari.launch()
        XCTAssertTrue(safari.windows.firstMatch.waitForExistence(timeout: 30))
        navigate(origin)
        click(safari, "Create New Identity")
        click(safari, "Continue")
        click(safari, "Admin", timeout: 180)
        waitForAdmissionCount(1)
        click(safari, "Server Settings")
        // Server Settings generates the invitation after loading a valid
        // public address. Use that actual UI result; the manual action is
        // available only when automatic generation has not succeeded.
        let copyInvite = safari.buttons["Copy Link"].firstMatch
        if !copyInvite.waitForExistence(timeout: 30) {
            click(safari, "Create Invite Link")
        }
        click(safari, "Copy Link")
        capture(safari, "founder-generated-and-copied-invite")
        let invite = try XCTUnwrap(NSPasteboard.general.string(forType: .string))
        let url = try XCTUnwrap(URL(string: invite))
        XCTAssertEqual(url.scheme, "https")
        XCTAssertTrue(["monolithannex.com", "www.monolithannex.com"].contains(url.host ?? ""))
        XCTAssertTrue(url.path.hasPrefix("/invite/"))
        var encoded = String(url.path.dropFirst("/invite/".count)).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        let payload = try JSONSerialization.jsonObject(with: XCTUnwrap(Data(base64Encoded: encoded))) as! [String: Any]
        XCTAssertEqual(payload["server"] as? String, origin)
        XCTAssertFalse((payload["code"] as? String ?? "").isEmpty)
        let inviteHash = SHA256.hash(data: Data(invite.utf8)).map { String(format: "%02x", $0) }.joined()
        events.append(["phase": "copied-public-invite-validated", "sha256": inviteHash, "destination": origin])

        click(safari, "Chat")
        selectGeneral(safari)
        capture(safari, "founder-ready-for-independent-message")
        safari.typeKey("t", modifierFlags: .command)
        navigate(invite)
        XCTAssertTrue(safari.buttons["Open in Annex"].waitForExistence(timeout: 60))
        XCTAssertTrue(textExists(safari, label))
        XCTAssertTrue(textExists(safari, origin))
        capture(safari, "actual-monolithannex-public-invite-page")
        openFromPublicPage()
        let coldPid = try XCTUnwrap(runningAnnex()?.processIdentifier)
        capture(annex, "installed-app-cold-launched-by-public-invite")
        waitForAdmissionCount(1)
        click(annex, "Create New Identity")
        XCTAssertTrue(annex.buttons["Join invite server"].waitForExistence(timeout: 90))
        XCTAssertTrue(textExists(annex, origin))
        capture(annex, "exact-destination-awaiting-explicit-join")
        waitForAdmissionCount(1)
        click(annex, "Join invite server")
        waitForAdmissionCount(2, timeout: 180)
        XCTAssertTrue(textExists(annex, slug, timeout: 90))
        XCTAssertTrue(textExists(annex, label))
        capture(annex, "installed-app-joined-correct-destination")
        selectGeneral(annex)
        let composer = annex.textViews.firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: 60))
        composer.click(); composer.typeText(message)
        click(annex, "Send")
        XCTAssertTrue(textExists(annex, message))
        capture(annex, "native-recipient-message-sent")

        safari.activate()
        safari.typeKey("1", modifierFlags: .command)
        XCTAssertTrue(textExists(safari, message, timeout: 60))
        capture(safari, "independent-founder-received-native-message")
        safari.typeKey("2", modifierFlags: .command)
        XCTAssertTrue(safari.buttons["Open in Annex"].waitForExistence(timeout: 30))
        openFromPublicPage()
        XCTAssertTrue(annex.buttons["Join invite server"].waitForExistence(timeout: 30))
        XCTAssertEqual(runningAnnex()?.processIdentifier, coldPid)
        XCTAssertTrue(textExists(annex, origin))
        capture(annex, "warm-public-invite-same-native-process")
        waitForAdmissionCount(2)
        click(annex, "Ignore")
        let receipt: [String: Any] = ["passed": true, "destination": origin, "server_slug": slug,
            "server_label": label, "invite_sha256": inviteHash, "native_pid": coldPid,
            "message": message, "cold_launch_by_public_link": true, "explicit_join": true,
            "independent_founder_received_message": true, "warm_delivery_same_process": true,
            "native_app_launched_directly_by_test": false]
        try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted, .sortedKeys])
            .write(to: evidence.appendingPathComponent("native-invite-result.json"))
        print("ANNEX_PUBLISHED_MACOS_INVITE_JOIN_MESSAGE_PASSED")
    }
}
