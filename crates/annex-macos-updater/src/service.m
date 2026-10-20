// The service is private to its containing app. Authenticate the audit-token-
// bound XPC message, not a caller-supplied PID, path, or claimed identifier.
#import <Foundation/Foundation.h>
#import <Security/Security.h>
#import <xpc/xpc.h>

extern int annex_install_signed_archive(const unsigned char *, size_t,
                                        const char *, const char *);
static SecRequirementRef callerRequirement;

static NSURL *applicationURL(void) {
    NSURL *url = [[NSBundle mainBundle] bundleURL]; // .../Installer.xpc
    for (int i = 0; i < 3; i++) url = [url URLByDeletingLastPathComponent];
    return url; // .../Annex.app
}

static BOOL prepareCallerRequirement(void) {
    SecStaticCodeRef code = NULL;
    if (SecStaticCodeCreateWithPath((CFURLRef)applicationURL(), kSecCSDefaultFlags, &code) != errSecSuccess)
        return NO;
    OSStatus valid = SecStaticCodeCheckValidity(code, kSecCSStrictValidate, NULL);
    OSStatus requirement = valid == errSecSuccess
        ? SecCodeCopyDesignatedRequirement(code, kSecCSDefaultFlags, &callerRequirement)
        : valid;
    if (requirement != errSecSuccess) NSLog(@"Annex installer caller requirement refused (%d)", (int)requirement);
    CFRelease(code);
    return requirement == errSecSuccess;
}

static BOOL authenticate(xpc_object_t message) {
    SecCodeRef sender = NULL;
    // Public on macOS 11+: the kernel-supplied message identity prevents PID
    // reuse and another sandboxed app from impersonating the installed client.
    OSStatus status = SecCodeCreateWithXPCMessage(message, kSecCSDefaultFlags, &sender);
    if (status != errSecSuccess) return NO;
    status = SecCodeCheckValidity(sender, kSecCSStrictValidate, callerRequirement);
    CFRelease(sender);
    if (status != errSecSuccess) NSLog(@"Annex installer caller authentication refused (%d)", (int)status);
    return status == errSecSuccess;
}

static void receive(xpc_connection_t connection, xpc_object_t message) {
    if (xpc_get_type(message) != XPC_TYPE_DICTIONARY) return;
    xpc_object_t reply = xpc_dictionary_create_reply(message);
    if (!reply) return;
    int result = 1;
    @autoreleasepool {
        if (authenticate(message)) {
            xpc_object_t archive = xpc_dictionary_get_value(message, "archive");
            xpc_object_t signature = xpc_dictionary_get_value(message, "signature");
            xpc_object_t version = xpc_dictionary_get_value(message, "version");
            // No paths, public keys, executable commands or environment overrides.
            if (xpc_dictionary_get_count(message) == 3 &&
                archive && xpc_get_type(archive) == XPC_TYPE_DATA &&
                signature && xpc_get_type(signature) == XPC_TYPE_STRING &&
                version && xpc_get_type(version) == XPC_TYPE_STRING &&
                xpc_data_get_length(archive) > 0 &&
                xpc_data_get_length(archive) <= 1024UL * 1024UL * 1024UL &&
                xpc_string_get_length(signature) <= 8192 &&
                xpc_string_get_length(version) <= 128) {
                result = annex_install_signed_archive(
                    xpc_data_get_bytes_ptr(archive), xpc_data_get_length(archive),
                    xpc_string_get_string_ptr(signature), xpc_string_get_string_ptr(version));
            }
        }
    }
    xpc_dictionary_set_int64(reply, "result", result);
    xpc_connection_send_message(connection, reply);
    xpc_release(reply);
}

static void acceptConnection(xpc_connection_t connection) {
    xpc_connection_set_event_handler(connection, ^(xpc_object_t message) {
        receive(connection, message);
    });
    xpc_connection_resume(connection);
}

void annex_installer_service_main(void) {
    @autoreleasepool {
        if (!prepareCallerRequirement()) exit(70);
    }
    xpc_main(acceptConnection);
}

// Verify the complete staged code seal, and preserve an existing Developer ID
// team boundary. Ad-hoc qualification still requires the pinned archive signature.
int annex_validate_staged_bundle(const char *currentPath, const char *stagedPath) {
    @autoreleasepool {
        SecStaticCodeRef current = NULL, staged = NULL;
        NSURL *currentURL = [NSURL fileURLWithPath:[NSString stringWithUTF8String:currentPath]];
        NSURL *stagedURL = [NSURL fileURLWithPath:[NSString stringWithUTF8String:stagedPath]];
        OSStatus status = SecStaticCodeCreateWithPath((CFURLRef)currentURL, 0, &current);
        if (status == errSecSuccess)
            status = SecStaticCodeCreateWithPath((CFURLRef)stagedURL, 0, &staged);
        if (status == errSecSuccess)
            status = SecStaticCodeCheckValidity(staged, kSecCSStrictValidate | kSecCSCheckNestedCode, NULL);
        CFDictionaryRef oldInfo = NULL, newInfo = NULL;
        if (status == errSecSuccess)
            status = SecCodeCopySigningInformation(current, kSecCSSigningInformation, &oldInfo);
        if (status == errSecSuccess)
            status = SecCodeCopySigningInformation(staged, kSecCSSigningInformation, &newInfo);
        if (status == errSecSuccess) {
            CFStringRef oldTeam = CFDictionaryGetValue(oldInfo, kSecCodeInfoTeamIdentifier);
            CFStringRef newTeam = CFDictionaryGetValue(newInfo, kSecCodeInfoTeamIdentifier);
            if (oldTeam && (!newTeam || !CFEqual(oldTeam, newTeam))) status = errSecCSReqFailed;
        }
        if (oldInfo) CFRelease(oldInfo);
        if (newInfo) CFRelease(newInfo);
        if (current) CFRelease(current);
        if (staged) CFRelease(staged);
        if (status != errSecSuccess) NSLog(@"Annex installer staged seal refused (%d)", (int)status);
        return status == errSecSuccess ? 0 : 1;
    }
}

