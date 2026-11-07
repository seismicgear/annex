import XCTest
import AppKit
import CryptoKit

final class NativeHostAcceptance: XCTestCase {
    let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
    let safari = XCUIApplication(bundleIdentifier: "com.apple.Safari")
    var steps: [[String: Any]] = []
    var phase = "before-launch"
    var evidence: URL { URL(fileURLWithPath: ProcessInfo.processInfo.environment["ANNEX_EVIDENCE_DIR"]!) }

    override func setUpWithError() throws {
        continueAfterFailure = false
        monitorAnnexLocalNetworkPermission()
    }

    func capture(_ name: String, app: XCUIApplication? = nil) {
        phase = name
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name; shot.lifetime = .keepAlways; add(shot)
        let tree = XCTAttachment(string: (app ?? annex).debugDescription)
        tree.name = name + "-accessibility"; tree.lifetime = .keepAlways; add(tree)
        steps.append(["phase": name, "at": ISO8601DateFormatter().string(from: Date())])
    }

    override func tearDownWithError() throws {
        capture("host-final-" + phase)
        let result = XCTAttachment(data: try JSONSerialization.data(withJSONObject: steps, options: [.prettyPrinted]), uniformTypeIdentifier: "public.json")
        result.name = "annex-native-host-events"; result.lifetime = .keepAlways; add(result)
        annex.terminate()
    }

    func click(_ title: String, timeout: TimeInterval = 30) throws {
        let button = annex.buttons[title].firstMatch
        let deadline = Date().addingTimeInterval(timeout)
        var scrolls = 0
        repeat {
            // Consent can appear after Host has already rendered. Waiting for
            // a later control alone never triggers XCTest's interruption monitor.
            // Both native and WebView network processes may request it.
            for _ in 0..<3 {
                if !acceptVisibleAnnexLocalNetworkPermission() { break }
                annex.activate()
            }
            if button.exists && button.isEnabled && button.isHittable { break }
            let view = annex.webViews.firstMatch
            if button.exists && button.isEnabled && view.exists && scrolls < 4 {
                let visible = view.frame
                if button.frame.maxY > visible.maxY || button.frame.minY < visible.minY {
                    view.scroll(byDeltaX: 0, deltaY: button.frame.maxY > visible.maxY ? -300 : 300)
                    scrolls += 1
                    continue
                }
            }
            let ready = NSPredicate { _, _ in button.exists && button.isEnabled && button.isHittable }
            _ = XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: ready, object: nil)],
                              timeout: min(2, max(0, deadline.timeIntervalSinceNow)))
        } while Date() < deadline
        guard button.exists && button.isEnabled && button.isHittable else {
            capture("host-missing-" + title)
            XCTFail("Published app did not expose " + title + " at " + phase)
            throw NSError(domain: "AnnexHostAcceptance", code: 1)
        }
        button.click()
    }

    func databaseSnapshot() -> [String: Any]? {
        guard let raw = try? String(contentsOf: evidence.appendingPathComponent("host-observations.jsonl"), encoding: .utf8),
              let last = raw.split(separator: "\n").last,
              let data = String(last).data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    func assertFounderBadge() throws {
        try click("Identity", timeout: 210)
        XCTAssertTrue(annex.staticTexts["Server founder"].firstMatch.waitForExistence(timeout: 30),
                      "The actual profile must identify the host as Server founder")
        capture("host-founder-profile-visible")
        // Permission sheets can arrive after the founder badge is observed.
        // A one-shot Escape can be consumed by that sheet, leaving Identity
        // open and making the underlying Admin button correctly inaccessible.
        // The ordinary click helper re-observes consent while waiting and
        // scrolls to the actual dialog Close control.
        for _ in 0..<5 {
            try click("Close", timeout: 60)
            if annex.buttons["Admin"].firstMatch.waitForExistence(timeout: 3) { break }
            // A second OS sheet can arrive between the helper's observation
            // and click. Confirm the postcondition and re-observe consent
            // before retrying this idempotent dialog-close action.
            acceptVisibleAnnexLocalNetworkPermission()
            annex.activate()
        }
        XCTAssertTrue(annex.buttons["Admin"].firstMatch.waitForExistence(timeout: 10),
                      "Closing Identity after native consent must reveal Admin")
    }

    func publicSummary(_ origin: String) throws -> [String: Any] {
        // Chromium cannot register its Mach service from XCTest's container.
        // The ordinary workflow process performs the same independent network
        // probe and returns a correlated observation, without changing Annex.
        let processID = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.annex.desktop").first?.processIdentifier)
        var response: [String: Any]?
        let ready = NSPredicate { _, _ in
            guard let data = try? Data(contentsOf: self.evidence.appendingPathComponent("public-summary-response.json")),
                  let value = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  value["app_pid"] as? Int == Int(processID),
                  value["origin"] as? String == origin else { return false }
            response = value
            return true
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: ready, object: nil)], timeout: 75), .completed)
        let observed = try XCTUnwrap(response)
        XCTAssertEqual(observed["passed"] as? Bool, true, observed["error"] as? String ?? "Public probe failed")
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: observed), uniformTypeIdentifier: "public.json")
        attachment.name = "independent-public-summary-" + String(processID)
        attachment.lifetime = .keepAlways; add(attachment)
        return try XCTUnwrap(observed["summary"] as? [String: Any])
    }

    func testPublishedHostSetup() throws {
        // Launch the installed, unmodified release with its ordinary defaults.
        // No API-created founder, injected token, configured public URL or proxy.
        annex.launch()
        XCTAssertTrue(annex.windows.firstMatch.waitForExistence(timeout: 40))
        capture("host-fresh-install")
        var prerequisiteIdentityClick = false
        if annex.buttons["Create New Identity"].firstMatch.waitForExistence(timeout: 5) {
            prerequisiteIdentityClick = true
            try click("Create New Identity")
        }
        try click("Start Hosting", timeout: 60)
        capture("host-clicked-once")
        try assertFounderBadge()
        try click("Admin", timeout: 210)
        capture("host-admin-available")
        let founder = NSPredicate { _, _ in
            guard let rows = self.databaseSnapshot()?["identities"] as? [[String: Any]] else { return false }
            return rows.count == 1 && (rows[0]["can_moderate"] as? Int) == 1 && (rows[0]["can_invite"] as? Int) == 1
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: founder, object: nil)], timeout: 15), .completed)
        try click("Server Settings")
        capture("host-settings-without-manual-configuration")
        let inviteField = annex.textFields["Invite link"].firstMatch
        XCTAssertTrue(inviteField.waitForExistence(timeout: 45))
        let invite = try XCTUnwrap(inviteField.value as? String)
        try click("Copy Link", timeout: 45)
        XCTAssertTrue(annex.buttons["Copied!"].firstMatch.waitForExistence(timeout: 10))
        acceptVisibleAnnexLocalNetworkPermission()
        // Observe the user's actual paste journey. The XCTest runner's own
        // pasteboard is not a reliable observation of the GUI login session.
        // Never set the pasteboard or type the expected URL into Safari here.
        safari.launch()
        XCTAssertTrue(safari.windows.firstMatch.waitForExistence(timeout: 30))
        acceptVisibleAnnexLocalNetworkPermission()
        safari.activate()
        safari.typeKey("l", modifierFlags: .command)
        safari.typeKey("v", modifierFlags: .command)
        let pasted = NSPredicate { _, _ in
            self.safari.textFields.allElementsBoundByIndex.contains { ($0.value as? String) == invite }
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: pasted, object: nil)], timeout: 15), .completed,
                       "The app-generated invitation must paste into the real browser unchanged")
        capture("host-copied-invite-pasted-in-browser", app: safari)
        safari.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(safari.buttons["Open in Annex"].firstMatch.waitForExistence(timeout: 60))
        capture("host-copied-invite-live-public-page", app: safari)
        annex.activate()
        let url = try XCTUnwrap(URL(string: invite))
        XCTAssertEqual(url.scheme, "https")
        XCTAssertTrue(["monolithannex.com", "www.monolithannex.com"].contains(url.host ?? ""))
        XCTAssertTrue(url.path.hasPrefix("/invite/"))
        XCTAssertFalse(prerequisiteIdentityClick, "Host must provision its identity without a separate identity-creation click")

        var encoded = String(url.path.dropFirst("/invite/".count)).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        let payload = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(Data(base64Encoded: encoded))) as? [String: Any])
        let destination = try XCTUnwrap(payload["server"] as? String)
        let publicURL = try XCTUnwrap(URL(string: destination))
        XCTAssertEqual(publicURL.scheme, "https")
        XCTAssertEqual(publicURL.host, "www.monolithannex.com")
        XCTAssertNotNil(publicURL.path.range(of: "^/host/[a-f0-9]{64}$", options: .regularExpression))
        let before = try XCTUnwrap(databaseSnapshot())
        let servers = try XCTUnwrap(before["servers"] as? [[String: Any]])
        XCTAssertEqual(servers.count, 1)
        let server = try XCTUnwrap(servers.first)
        let slug = try XCTUnwrap(server["slug"] as? String)
        XCTAssertEqual(server["public_url"] as? String, destination)
        let summary = try publicSummary(destination)
        XCTAssertEqual(summary["slug"] as? String, slug,
                       "The public invite must reach the same server the EXE/app provisioned")
        XCTAssertEqual(summary["label"] as? String, server["label"] as? String)
        XCTAssertEqual(summary["public_url"] as? String, destination)
        capture("host-public-destination-verified")

        let identitiesBefore = try XCTUnwrap(before["identities"] as? [[String: Any]])
        let founderID = try XCTUnwrap(identitiesBefore.first?["id"] as? Int)
        annex.terminate()
        annex.launch()
        try assertFounderBadge()
        let retained = NSPredicate { _, _ in
            guard let snapshot = self.databaseSnapshot(),
                  let identities = snapshot["identities"] as? [[String: Any]],
                  let servers = snapshot["servers"] as? [[String: Any]],
                  identities.count == 1, servers.count == 1 else { return false }
            return (identities[0]["id"] as? Int) == founderID &&
                   (identities[0]["can_moderate"] as? Int) == 1 &&
                   (identities[0]["can_invite"] as? Int) == 1 &&
                   (servers[0]["slug"] as? String) == slug &&
                   (servers[0]["public_url"] as? String) == destination
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: retained, object: nil)], timeout: 15), .completed)
        XCTAssertEqual(try publicSummary(destination)["slug"] as? String, slug)
        capture("host-founder-and-public-address-retained-after-reopen")
        let receipt: [String: Any] = ["passed": true, "host_clicks": 1, "separate_identity_click": prerequisiteIdentityClick,
            "invite_sha256": SHA256.hash(data: Data(invite.utf8)).map { String(format: "%02x", $0) }.joined(),
            "routing_origin": url.host!, "destination": destination, "server_slug": slug,
            "founder_badge_visible": true, "founder_retained_after_reopen": true,
            "public_destination_verified": true, "copied_invite_verified_by_native_paste": true,
            "joined_recipient_proven": false]
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted]), uniformTypeIdentifier: "public.json")
        attachment.name = "annex-native-host-result"; attachment.lifetime = .keepAlways; add(attachment)
    }
}
