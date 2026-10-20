import XCTest
import AppKit
import CryptoKit

final class NativeUpdaterAcceptance: XCTestCase {
    let annex = XCUIApplication(bundleIdentifier: "com.annex.desktop")
    let binary = URL(fileURLWithPath: "/Applications/Annex.app/Contents/MacOS/annex-desktop")
    let marker = "Annex native Mac update preserves the founder and message"
    var evidence: URL { URL(fileURLWithPath: ProcessInfo.processInfo.environment["ANNEX_EVIDENCE_DIR"]!) }
    var stages: [[String: Any]] = []

    override func setUpWithError() throws {
        continueAfterFailure = false
        monitorAnnexLocalNetworkPermission()
    }

    func capture(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name; shot.lifetime = .keepAlways; add(shot)
        let tree = XCTAttachment(string: annex.debugDescription)
        tree.name = name + "-accessibility"; tree.lifetime = .keepAlways; add(tree)
    }

    override func tearDownWithError() throws {
        capture("updater-final")
        let record = XCTAttachment(data: try JSONSerialization.data(withJSONObject: stages, options: [.prettyPrinted]),
                                   uniformTypeIdentifier: "public.json")
        record.name = "updater-stages"; record.lifetime = .keepAlways; add(record)
        annex.terminate()
    }

    func hash() throws -> String {
        SHA256.hash(data: try Data(contentsOf: binary)).map { String(format: "%02x", $0) }.joined()
    }

    func click(_ title: String, timeout: TimeInterval = 60) throws {
        let button = annex.buttons[title].firstMatch
        let expires = Date().addingTimeInterval(timeout)
        var scrolls = 0
        repeat {
            for _ in 0..<3 {
                if !acceptVisibleAnnexLocalNetworkPermission() { break }
                annex.activate()
            }
            if button.exists && button.isEnabled && button.isHittable { button.click(); return }
            let view = annex.webViews.firstMatch
            if button.exists && button.isEnabled && view.exists && scrolls < 5 {
                let visible = view.frame
                if button.frame.maxY > visible.maxY || button.frame.minY < visible.minY {
                    view.scroll(byDeltaX: 0, deltaY: button.frame.maxY > visible.maxY ? -300 : 300)
                    scrolls += 1
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        } while Date() < expires
        capture("updater-missing-" + title)
        XCTFail("Native updater did not expose enabled control: " + title)
        throw NSError(domain: "AnnexUpdaterAcceptance", code: 1)
    }

    func text(_ value: String, timeout: TimeInterval = 60, contains: Bool = false) -> Bool {
        let expires = Date().addingTimeInterval(timeout)
        repeat {
            if acceptVisibleAnnexLocalNetworkPermission() { annex.activate() }
            // WebKit also exposes numeric StaticText values (for example a
            // heading's level). NSPredicate's CONTAINS operator throws on those
            // values; inspect only actual strings from the native AX tree.
            for element in annex.staticTexts.allElementsBoundByIndex {
                let strings = [element.label, element.value as? String].compactMap { $0 }
                if strings.contains(where: { contains ? $0.contains(value) : $0 == value }) {
                    return true
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        } while Date() < expires
        return false
    }

    func version(_ kind: String, equals expected: String, recheckAfterConsent: Bool = false) -> Bool {
        // WebKit exposes the paragraph's literal and interpolated version as
        // two sibling StaticText nodes. Require that exact pair inside the
        // actual Updates dialog, never a version elsewhere in the window.
        let dialog = annex.groups.matching(NSPredicate(
            format: "label == %@ OR title == %@", "Annex Updates", "Annex Updates")).firstMatch
        let expires = Date().addingTimeInterval(60)
        var recheck = false
        repeat {
            if acceptVisibleAnnexLocalNetworkPermission() {
                annex.activate()
                recheck = recheckAfterConsent
            }
            if dialog.exists {
                for group in dialog.groups.allElementsBoundByIndex {
                    let values = group.children(matching: .staticText).allElementsBoundByIndex.map {
                        (($0.value as? String) ?? $0.label).trimmingCharacters(in: .whitespacesAndNewlines)
                    }
                    if values == [kind + " version:", expected] { return true }
                }
            }
            // A newly displayed consent dialog can consume the Check click:
            // the actual updater remains idle and the fixture receives no
            // request. After granting the observed consent, repeat only that
            // idempotent action if its real control is enabled. Never retry a
            // download or installation, or treat a click as a successful check.
            let check = dialog.buttons["Check for updates"].firstMatch
            if recheck && check.exists && check.isEnabled && check.isHittable {
                check.click()
                recheck = false
                capture("updater-check-retried-after-local-network-consent")
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        } while Date() < expires
        capture("updater-missing-" + kind + "-version")
        return false
    }

    func selectGeneral() throws {
        let general = annex.buttons.matching(NSPredicate(format: "label CONTAINS %@ OR title CONTAINS %@", "General", "General")).firstMatch
        XCTAssertTrue(general.waitForExistence(timeout: 60))
        general.click()
        let join = annex.buttons["Join channel"].firstMatch
        if join.waitForExistence(timeout: 3) { join.click() }
    }

    func founderAndMessage(create: Bool) throws {
        XCTAssertTrue(annex.windows.firstMatch.waitForExistence(timeout: 40))
        if create {
            // First-launch resources and local-network consent can exceed five
            // seconds. Never infer that hosting already started from a missing
            // control during that interval; perform the real Host action.
            try click("Start Hosting", timeout: 90)
            capture("updater-baseline-host-clicked")
        } else {
            let host = annex.buttons["Start Hosting"].firstMatch
            let identity = annex.buttons["Identity"].firstMatch
            let expires = Date().addingTimeInterval(90)
            while !host.exists && !identity.exists && Date() < expires {
                if acceptVisibleAnnexLocalNetworkPermission() { annex.activate() }
                RunLoop.current.run(until: Date().addingTimeInterval(0.5))
            }
            if host.exists { try click("Start Hosting") }
        }
        try click("Identity", timeout: 210)
        XCTAssertTrue(text("Server founder"))
        capture(create ? "updater-baseline-founder" : "updater-recovered-founder")
        for _ in 0..<5 {
            try click("Close")
            if annex.buttons["Admin"].firstMatch.waitForExistence(timeout: 3) { break }
            acceptVisibleAnnexLocalNetworkPermission()
        }
        try click("Chat")
        try selectGeneral()
        if create {
            let composer = annex.textViews.firstMatch
            XCTAssertTrue(composer.waitForExistence(timeout: 60))
            // First-use local-network consent can arrive in the middle of a
            // long synthesized typing event. Use the ordinary native Paste
            // action and verify the visible field before submitting anything.
            // This supplies only the test message, never app/profile state.
            let expires = Date().addingTimeInterval(60)
            repeat {
                if acceptVisibleAnnexLocalNetworkPermission() { annex.activate() }
                if composer.exists && composer.isEnabled && composer.isHittable {
                    composer.click()
                    NSPasteboard.general.clearContents()
                    XCTAssertTrue(NSPasteboard.general.setString(marker, forType: .string))
                    composer.typeKey("a", modifierFlags: .command)
                    composer.typeKey("v", modifierFlags: .command)
                    if (composer.value as? String) == marker { break }
                }
                RunLoop.current.run(until: Date().addingTimeInterval(0.5))
            } while Date() < expires
            XCTAssertEqual(composer.value as? String, marker, "The native composer must contain the complete test message")
            try click("Send")
        }
        XCTAssertTrue(text(marker))
    }

    func transport(_ mode: String) throws {
        var request = URLRequest(url: URL(string: "https://localhost:34567/control")!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer " + ProcessInfo.processInfo.environment["ANNEX_UPDATER_CONTROL_TOKEN"]!,
                         forHTTPHeaderField: "Authorization")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["mode": mode])
        let done = expectation(description: "External transport mode selected")
        var status: Int?
        URLSession.shared.dataTask(with: request) { _, response, _ in
            status = (response as? HTTPURLResponse)?.statusCode
            done.fulfill()
        }.resume()
        wait(for: [done], timeout: 15)
        XCTAssertEqual(status, 204, "Only the independent HTTPS transport fixture is controlled")
    }

    func testRealUpdaterReplacementAndRecovery() throws {
        let configuration = try XCTUnwrap(try JSONSerialization.jsonObject(
            with: Data(contentsOf: evidence.appendingPathComponent("updater-expected.json"))) as? [String: Any])
        let expectedHash = try XCTUnwrap(configuration["executableSha256"] as? String)
        let before = try hash()
        XCTAssertNotEqual(before, expectedHash)
        annex.launch()
        try founderAndMessage(create: true)
        let initialPID = try XCTUnwrap(NSRunningApplication.runningApplications(withBundleIdentifier: "com.annex.desktop").first?.processIdentifier)
        try click("Updates")
        XCTAssertTrue(version("Installed", equals: "0.1.2"))
        try click("Check for updates")
        XCTAssertTrue(version("Available", equals: "0.1.3", recheckAfterConsent: true))
        for mode in ["stall", "corrupt", "normal"] {
            try transport(mode)
            try click("Download update")
            if mode == "normal" {
                XCTAssertTrue(text("Signature verified. Ready to install.", timeout: 180))
            } else {
                let message = mode == "stall" ? "stopped receiving data" : "signature could not be verified"
                XCTAssertTrue(text(message, timeout: 150, contains: true))
            }
            XCTAssertEqual(try hash(), before, "Downloads must not modify the installed app")
            stages.append(["mode": mode, "installedBinaryUnchanged": true])
            capture("updater-" + mode)
        }
        try click("Install and restart")
        XCTAssertTrue(text("Installing closes active calls", contains: true))
        try click("Confirm install and restart")
        let replaced = NSPredicate { _, _ in
            guard (try? self.hash()) == expectedHash else { return false }
            let applications = NSRunningApplication.runningApplications(withBundleIdentifier: "com.annex.desktop")
            return applications.count == 1 && applications[0].processIdentifier != initialPID
        }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: replaced, object: nil)], timeout: 180), .completed)
        // Observe the actual restarted process; never launch a replacement on
        // the installer's behalf and never prepopulate its profile.
        annex.activate()
        try founderAndMessage(create: false)
        try click("Updates")
        XCTAssertTrue(version("Installed", equals: "0.1.3"))
        capture("updater-recovered-installed-candidate")
        let receipt: [String: Any] = [
            "passed": true, "candidateSource": configuration["sourceCommit"]!,
            "installedExecutableSha256": expectedHash, "baselineVersion": "0.1.2",
            "baselineIsPublished012": false, "baselineBuildOverrides": ["version", "updater HTTPS fixture endpoint"],
            "actualInAppUpdater": true, "actualAppBundleReplacement": true, "automaticRestart": true,
            "ordinaryProfile": true, "founderAndMessagePreserved": true, "stalledDownloadRecovered": true,
            "corruptSignatureRejected": true, "productionOriginTested": false,
            "developerIdAndNotarizationTested": false]
        let attachment = XCTAttachment(data: try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted]),
                                       uniformTypeIdentifier: "public.json")
        attachment.name = "annex-native-updater-result"; attachment.lifetime = .keepAlways; add(attachment)
    }
}
