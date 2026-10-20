// Native negative test: the authorized main executable reaches signature
// rejection; a separately signed sibling must fail caller authentication.
#import <Foundation/Foundation.h>
#import <xpc/xpc.h>

int main(void) {
    @autoreleasepool {
        NSLog(@"Annex installer boundary probe variant %d", ANNEX_PROBE_VARIANT);
        xpc_connection_t connection = xpc_connection_create("com.annex.desktop.installer", NULL);
        xpc_connection_set_event_handler(connection, ^(xpc_object_t event) { (void)event; });
        xpc_connection_resume(connection);
        xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
        xpc_dictionary_set_data(request, "archive", "untrusted", 9);
        xpc_dictionary_set_string(request, "signature", "untrusted");
        xpc_dictionary_set_string(request, "version", "0.1.3");
        xpc_object_t reply = xpc_connection_send_message_with_reply_sync(connection, request);
        int result = 99;
        if (xpc_get_type(reply) == XPC_TYPE_DICTIONARY) {
            xpc_object_t value = xpc_dictionary_get_value(reply, "result");
            if (value && xpc_get_type(value) == XPC_TYPE_INT64)
                result = (int)xpc_int64_get_value(value);
        }
        printf("%d\n", result);
        xpc_release(reply);
        xpc_release(request);
        xpc_connection_cancel(connection);
        xpc_release(connection);
        return 0;
    }
}
