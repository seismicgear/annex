// The renderer never supplies a path, service name, key or command. The
// installed bundle determines the service; both peers authenticate XPC
// messages using the kernel-bound code identity.
#import <Foundation/Foundation.h>
#import <Security/Security.h>
#import <xpc/xpc.h>

int annex_macos_install(const unsigned char *bytes, size_t length,
                        const char *signature, const char *version) {
    @autoreleasepool {
        NSURL *service = [[[NSBundle mainBundle] bundleURL]
            URLByAppendingPathComponent:@"Contents/XPCServices/com.annex.desktop.installer.xpc"];
        SecStaticCodeRef code = NULL;
        SecRequirementRef requirement = NULL;
        OSStatus status = SecStaticCodeCreateWithPath((CFURLRef)service, 0, &code);
        if (status == errSecSuccess)
            status = SecStaticCodeCheckValidity(code, kSecCSStrictValidate, NULL);
        if (status == errSecSuccess)
            status = SecCodeCopyDesignatedRequirement(code, 0, &requirement);
        if (code) CFRelease(code);
        if (status != errSecSuccess || !requirement) return 1;

        dispatch_queue_t queue = dispatch_queue_create("com.annex.desktop.install-reply", DISPATCH_QUEUE_SERIAL);
        dispatch_semaphore_t completed = dispatch_semaphore_create(0);
        xpc_connection_t connection = xpc_connection_create("com.annex.desktop.installer", queue);
        if (!connection) {
            CFRelease(requirement);
            dispatch_release(queue);
            dispatch_release(completed);
            return 1;
        }
        xpc_connection_set_event_handler(connection, ^(xpc_object_t event) {
            // Request failures are delivered to its reply block as XPC errors.
            (void)event;
        });
        xpc_connection_resume(connection);
        xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
        xpc_dictionary_set_data(request, "archive", bytes, length);
        xpc_dictionary_set_string(request, "signature", signature);
        xpc_dictionary_set_string(request, "version", version);

        // Blocks copy __block storage to the heap. The callback owns a second
        // semaphore reference, so timeout cannot leave a dangling stack
        // pointer or destroy an object still used by a queued reply.
        __block int result = 1;
        dispatch_retain(completed);
        xpc_connection_send_message_with_reply(connection, request, queue, ^(xpc_object_t reply) {
            if (xpc_get_type(reply) == XPC_TYPE_DICTIONARY) {
                SecCodeRef sender = NULL;
                if (SecCodeCreateWithXPCMessage(reply, 0, &sender) == errSecSuccess) {
                    if (SecCodeCheckValidity(sender, 0, requirement) == errSecSuccess) {
                        xpc_object_t value = xpc_dictionary_get_value(reply, "result");
                        if (value && xpc_get_type(value) == XPC_TYPE_INT64)
                            result = xpc_int64_get_value(value) == 0 ? 0 : 2;
                    }
                    CFRelease(sender);
                }
            }
            CFRelease(requirement);
            dispatch_semaphore_signal(completed);
            dispatch_release(completed);
        });
        xpc_release(request);
        long timedOut = dispatch_semaphore_wait(completed,
            dispatch_time(DISPATCH_TIME_NOW, 300LL * NSEC_PER_SEC));
        int outcome = timedOut ? 3 : result;
        xpc_connection_cancel(connection);
        xpc_release(connection);
        dispatch_release(completed);
        dispatch_release(queue);
        return outcome;
    }
}
