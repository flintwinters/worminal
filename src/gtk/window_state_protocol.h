#ifndef WORMINAL_WINDOW_STATE_PROTOCOL_H
#define WORMINAL_WINDOW_STATE_PROTOCOL_H
/* Public read-only contract shared by both GTK implementations. */
#define WORMINAL_STATE_PATH "/org/worminal/Windows"
#define WORMINAL_STATE_INTERFACE "org.worminal.Windows1"
#define WORMINAL_STATE_WINDOWS "a(ubbays)"
#define WORMINAL_STATE_PREFIX "org.worminal.Instance.p"
#define WORMINAL_STATE_METHOD "GetWindows"
#define WORMINAL_STATE_SIGNAL "Changed"
#define WORMINAL_STATE_XML "<node><interface name='" WORMINAL_STATE_INTERFACE "'>" \
    "<method name='" WORMINAL_STATE_METHOD "'><arg direction='out' type='" WORMINAL_STATE_WINDOWS "'/></method>" \
    "<signal name='" WORMINAL_STATE_SIGNAL "'><arg type='" WORMINAL_STATE_WINDOWS "'/></signal>" \
    "</interface></node>"
#endif
