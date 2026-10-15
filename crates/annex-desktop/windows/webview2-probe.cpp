#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <objbase.h>
#include <msi.h>
#include <msiquery.h>
#include <cwchar>

// Linked to the unmodified Microsoft SDK loader. A registry version alone
// does not establish availability: Windows Sandbox can inherit stale entries.
extern "C" HRESULT STDAPICALLTYPE GetAvailableCoreWebView2BrowserVersionString(
    PCWSTR browserExecutableFolder, PWSTR* versionInfo);

namespace {
constexpr int available = 0;
constexpr int missing = 2;
constexpr int probeError = 3;

int inspect(PWSTR* version) {
    *version = nullptr;
    const HRESULT result = GetAvailableCoreWebView2BrowserVersionString(nullptr, version);
    if (result == HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)) return missing;
    if (FAILED(result)) return probeError;
    if (!*version || !**version) return missing;
    // The API can return a preview Edge channel. Require the Evergreen Runtime
    // version form, not an installed Beta/Dev/Canary browser as a substitute.
    unsigned int components = 0;
    bool digit = false;
    for (const wchar_t* p = *version; *p; ++p) {
        if (*p >= L'0' && *p <= L'9') digit = true;
        else if (*p == L'.' && digit) { ++components; digit = false; }
        else return missing;
    }
    if (components != 3 || !digit || wcscmp(*version, L"0.0.0.0") == 0) return missing;
    return available;
}

void release(PWSTR version) { if (version) CoTaskMemFree(version); }

UINT reportFailure(MSIHANDLE install) {
    const MSIHANDLE record = MsiCreateRecord(0);
    if (record) {
        MsiRecordSetStringW(record, 0, L"Annex cannot continue because the Microsoft WebView2 Runtime is unavailable after setup. Repair the runtime and retry installation.");
        MsiProcessMessage(install, INSTALLMESSAGE_ERROR, record);
        MsiCloseHandle(record);
    }
    return ERROR_INSTALL_FAILURE;
}
}

extern "C" int WINAPI AnnexWebView2Probe() {
    PWSTR version = nullptr;
    const int result = inspect(&version);
    release(version);
    return result;
}

// Runs after AppSearch and before the deferred Evergreen installation action.
// Replaces Tauri's registry-only result; never edits Windows registration.
extern "C" UINT WINAPI AnnexWebView2MsiDetect(MSIHANDLE install) {
    PWSTR version = nullptr;
    const int result = inspect(&version);
    const UINT setResult = result == probeError ? ERROR_INSTALL_FAILURE :
        MsiSetPropertyW(install, L"INSTALLED_WEBVIEW2_VERSION", result == available ? version : L"");
    release(version);
    return setResult == ERROR_SUCCESS ? ERROR_SUCCESS : reportFailure(install);
}

// Runs after the deferred Evergreen action and refuses false completion even
// when Microsoft's installer returned success without a usable runtime.
extern "C" UINT WINAPI AnnexWebView2MsiVerify(MSIHANDLE install) {
    return AnnexWebView2Probe() == available ? ERROR_SUCCESS : reportFailure(install);
}
