"""Create a standalone XCTest UI bundle; never rebuild or instrument Annex."""
from pathlib import Path
from xml.sax.saxutils import quoteattr
import sys
import shutil
import os

root = Path(__file__).resolve().parent
source = sys.argv[1] if len(sys.argv) == 2 else 'NativeAcceptance.swift'
if source not in ('NativeAcceptance.swift', 'NativeHostAcceptance.swift', 'PublicInviteAcceptance.swift', 'NativeUpdaterAcceptance.swift'):
    raise SystemExit('Unknown native test source')
project = root / 'NativeAcceptance.xcodeproj'
project.mkdir(exist_ok=False)
(project / 'project.pbxproj').write_text('''// !$*UTF8*$!
{
 archiveVersion = 1; classes = {}; objectVersion = 56;
 objects = {
  A00000000000000000000001 = {isa = PBXProject; attributes = {LastUpgradeCheck = 1600;}; buildConfigurationList = A00000000000000000000002; compatibilityVersion = "Xcode 14.0"; developmentRegion = en; knownRegions = (en, Base); mainGroup = A00000000000000000000003; productRefGroup = A00000000000000000000004; projectDirPath = ""; projectRoot = ""; targets = (A00000000000000000000005); };
  A00000000000000000000002 = {isa = XCConfigurationList; buildConfigurations = (A00000000000000000000006); defaultConfigurationIsVisible = 0; defaultConfigurationName = Debug;};
  A00000000000000000000003 = {isa = PBXGroup; children = (A00000000000000000000007,A0000000000000000000000E,A00000000000000000000004); sourceTree = "<group>";};
  A00000000000000000000004 = {isa = PBXGroup; children = (A00000000000000000000008); name = Products; sourceTree = "<group>";};
  A00000000000000000000005 = {isa = PBXNativeTarget; buildConfigurationList = A00000000000000000000009; buildPhases = (A0000000000000000000000A,A0000000000000000000000B); buildRules = (); dependencies = (); name = NativeAcceptance; productName = NativeAcceptance; productReference = A00000000000000000000008; productType = "com.apple.product-type.bundle.ui-testing";};
  A00000000000000000000006 = {isa = XCBuildConfiguration; buildSettings = {SDKROOT = macosx; MACOSX_DEPLOYMENT_TARGET = 13.0; CLANG_ENABLE_MODULES = YES; SWIFT_VERSION = 5.0;}; name = Debug;};
  A00000000000000000000007 = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = NativeAcceptance.swift; sourceTree = "<group>";};
  A00000000000000000000008 = {isa = PBXFileReference; explicitFileType = wrapper.cfbundle; includeInIndex = 0; path = NativeAcceptance.xctest; sourceTree = BUILT_PRODUCTS_DIR;};
  A00000000000000000000009 = {isa = XCConfigurationList; buildConfigurations = (A0000000000000000000000C); defaultConfigurationIsVisible = 0; defaultConfigurationName = Debug;};
  A0000000000000000000000A = {isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (A0000000000000000000000D,A0000000000000000000000F); runOnlyForDeploymentPostprocessing = 0;};
  A0000000000000000000000B = {isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0;};
  A0000000000000000000000C = {isa = XCBuildConfiguration; buildSettings = {CODE_SIGN_IDENTITY = "-"; CODE_SIGN_STYLE = Manual; GENERATE_INFOPLIST_FILE = YES; PRODUCT_BUNDLE_IDENTIFIER = com.annex.acceptance.NativeAcceptance; PRODUCT_NAME = "$(TARGET_NAME)"; SWIFT_VERSION = 5.0; MACOSX_DEPLOYMENT_TARGET = 13.0; ENABLE_APP_SANDBOX = NO;}; name = Debug;};
  A0000000000000000000000D = {isa = PBXBuildFile; fileRef = A00000000000000000000007;};
  A0000000000000000000000E = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = NativePermissionHandling.swift; sourceTree = "<group>";};
  A0000000000000000000000F = {isa = PBXBuildFile; fileRef = A0000000000000000000000E;};
 }; rootObject = A00000000000000000000001;
}
'''.replace('path = NativeAcceptance.swift;', f'path = {source};'))
schemes = project / 'xcshareddata/xcschemes'
schemes.mkdir(parents=True)
reference = '<BuildableReference BuildableIdentifier="primary" BlueprintIdentifier="A00000000000000000000005" BuildableName="NativeAcceptance.xctest" BlueprintName="NativeAcceptance" ReferencedContainer="container:NativeAcceptance.xcodeproj"/>'
evidence = Path('native-evidence').resolve()
probe_environment = {}
if source == 'NativeUpdaterAcceptance.swift':
    token = os.environ['ANNEX_UPDATER_CONTROL_TOKEN']
    assert len(token) >= 32
    probe_environment['ANNEX_UPDATER_CONTROL_TOKEN'] = token
if os.environ.get('PLAYWRIGHT_BROWSERS_PATH'):
    # XCTest gives its runner a container HOME. Keep the independent probe on
    # the same explicit browser installation used by the workflow's setup step.
    probe_environment['PLAYWRIGHT_BROWSERS_PATH'] = os.environ['PLAYWRIGHT_BROWSERS_PATH']
test_environment = ''.join(f'<EnvironmentVariable key={quoteattr(key)} value={quoteattr(str(value))} isEnabled="YES"/>'
    for key, value in {'ANNEX_EVIDENCE_DIR': evidence, 'ANNEX_NATIVE_FIXTURE': evidence / 'fixture.json', 'ANNEX_PROBE_NODE': shutil.which('node') or '/usr/bin/false', 'ANNEX_SUMMARY_PROBE': root.parent / 'public-host-summary.mjs', **probe_environment}.items())
(schemes / 'NativeAcceptance.xcscheme').write_text(f'''<?xml version="1.0" encoding="UTF-8"?>
<Scheme LastUpgradeVersion="1600" version="1.3">
 <BuildAction parallelizeBuildables="YES" buildImplicitDependencies="YES"><BuildActionEntries><BuildActionEntry buildForTesting="YES" buildForRunning="NO" buildForProfiling="NO" buildForArchiving="NO" buildForAnalyzing="YES">{reference}</BuildActionEntry></BuildActionEntries></BuildAction>
 <TestAction buildConfiguration="Debug" selectedDebuggerIdentifier="Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier="Xcode.IDEFoundation.Launcher.LLDB" shouldUseLaunchSchemeArgsEnv="NO"><Testables><TestableReference skipped="NO">{reference}</TestableReference></Testables><EnvironmentVariables>{test_environment}</EnvironmentVariables></TestAction>
</Scheme>
''')
