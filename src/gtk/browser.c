/* Internal WebKit views move between local windows; profiles stay on this machine. */
#include <webkit2/webkit2.h>
#include "preview.h"
#include "browser_module.h"
#include "browser_target.h"
#include "build/file_explorer.h"

struct Browser {
	App *app;
	Page *page;
	GtkWidget *box, *entry, *notice;
	WebKitWebView *view;
};
static WebKitWebContext *context;

static void
notice(Browser *b, const char *message)
{
	gtk_label_set_text(GTK_LABEL(b->notice), message ? message : "");
	gtk_widget_set_visible(b->notice, message != NULL);
}

static void
location(GtkEntry *entry, Browser *b)
{
	const char *text = gtk_entry_get_text(entry);
	if (!*text) return;
	char *url = g_uri_peek_scheme(text) ? g_strdup(text) : g_strconcat("https://", text, NULL);
	webkit_web_view_load_uri(b->view, url);
	g_free(url);
	gtk_widget_hide(b->entry);
	gtk_widget_grab_focus(GTK_WIDGET(b->view));
}

static void
changed(WebKitWebView *view, GParamSpec *property, Browser *b)
{
	(void)property;
	const char *url = webkit_web_view_get_uri(view);
	const char *title = webkit_web_view_get_title(view);
	if (url && !gtk_widget_has_focus(b->entry))
		gtk_entry_set_text(GTK_ENTRY(b->entry), url);
	if (b->page) {
		app_page_title(b->page, title && *title ? title : url ? url : "Browser");
		if (url && property && !strcmp(property->name, "uri") && g_strcmp0(url, b->page->url)) {
			g_free(b->page->url);
			b->page->url = g_strdup(url);
			WireBuffer buffer = {0};
			wire_put_string(&buffer, url);
			app_send(b->app, WIRE_BROWSER_NAVIGATE, b->page->id, buffer.data, buffer.len);
			wire_buffer_free(&buffer);
		}
	}
}

static void
load_changed(WebKitWebView *view, WebKitLoadEvent event, Browser *b)
{
	if (event == WEBKIT_LOAD_STARTED) notice(b, NULL);
	changed(view, NULL, b);
}

static gboolean
load_failed(WebKitWebView *view, WebKitLoadEvent event, const char *url, GError *error, Browser *b)
{
	(void)view; (void)event; (void)url;
	if (!g_error_matches(error, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED))
		notice(b, error->message);
	return TRUE;
}

static gboolean
tls_failed(WebKitWebView *view, const char *url, GTlsCertificate *certificate,
           GTlsCertificateFlags errors, Browser *b)
{
	(void)view; (void)certificate;
	char *message = g_strdup_printf("Certificate validation failed (%u) for %s. Connection blocked.", errors, url);
	notice(b, message);
	g_free(message);
	return TRUE;
}

static void
renderer_failed(WebKitWebView *view, WebKitWebProcessTerminationReason reason, Browser *b)
{
	(void)view; (void)reason;
	notice(b, "The page renderer stopped. Press Ctrl+R to reopen this page.");
}

static GtkWidget *
popup(WebKitWebView *view, WebKitNavigationAction *action, Browser *b)
{
	(void)view;
	if (webkit_navigation_action_is_user_gesture(action))
		app_browser(b->app, webkit_uri_request_get_uri(webkit_navigation_action_get_request(action)));
	return NULL;
}

static void
close_requested(WebKitWebView *view, Browser *b)
{
	(void)view;
	if (b->page) app_close(b->page);
}

static gboolean
policy(WebKitWebView *view, WebKitPolicyDecision *decision, WebKitPolicyDecisionType type, Browser *b)
{
	(void)view; (void)b;
	if (type == WEBKIT_POLICY_DECISION_TYPE_RESPONSE) {
		if (!webkit_response_policy_decision_is_mime_type_supported(WEBKIT_RESPONSE_POLICY_DECISION(decision))) {
			webkit_policy_decision_download(decision);
			return TRUE;
		}
	} else if (type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION ||
	           type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
		WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(
		    WEBKIT_NAVIGATION_POLICY_DECISION(decision));
		const char *url = webkit_uri_request_get_uri(webkit_navigation_action_get_request(action));
		if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
			/* A shared popup is a saved link with an independent local page, just
			 * like any other browser tab. No cross-tab window.opener relationship. */
			if (webkit_navigation_action_is_user_gesture(action)) app_browser(b->app, url);
			webkit_policy_decision_ignore(decision);
			return TRUE;
		}
		const char *scheme = g_uri_peek_scheme(url);
		if (!scheme || (g_ascii_strcasecmp(scheme, "http") && g_ascii_strcasecmp(scheme, "https") &&
		                g_ascii_strcasecmp(scheme, "about") && g_ascii_strcasecmp(scheme, "file") &&
		                g_ascii_strcasecmp(scheme, "data") && g_ascii_strcasecmp(scheme, "blob") &&
		                g_ascii_strcasecmp(scheme, WORKSPACE_BROWSER_SCHEME))) {
			webkit_policy_decision_ignore(decision);
			notice(b, "This URL scheme is unsupported.");
			return TRUE;
		}
	}
	return FALSE;
}

static gboolean
fullscreen(WebKitWebView *view, Browser *b)
{
	(void)view;
	notice(b, "Page fullscreen requests are unsupported in this preview.");
	return TRUE;
}

static void
permission_response(GtkDialog *dialog, int response, WebKitPermissionRequest *request)
{
	if (response == GTK_RESPONSE_ACCEPT) webkit_permission_request_allow(request);
	else webkit_permission_request_deny(request);
	g_object_set_data(G_OBJECT(dialog), "answered", GINT_TO_POINTER(1));
	gtk_widget_destroy(GTK_WIDGET(dialog));
}

static void
permission_destroy(GtkWidget *dialog, WebKitPermissionRequest *request)
{
	if (!g_object_get_data(G_OBJECT(dialog), "answered")) webkit_permission_request_deny(request);
	g_object_unref(request);
}

static gboolean
permission(WebKitWebView *view, WebKitPermissionRequest *request, Browser *b)
{
	const char *what = NULL;
	if (WEBKIT_IS_GEOLOCATION_PERMISSION_REQUEST(request)) what = "your location";
	else if (WEBKIT_IS_USER_MEDIA_PERMISSION_REQUEST(request)) what = "your camera or microphone";
	else if (WEBKIT_IS_NOTIFICATION_PERMISSION_REQUEST(request)) what = "desktop notifications";
	if (!what) {
		webkit_permission_request_deny(request);
		return TRUE;
	}
	GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(b->app->window),
	    GTK_DIALOG_DESTROY_WITH_PARENT, GTK_MESSAGE_QUESTION, GTK_BUTTONS_NONE,
	    "Allow %s to access %s?", webkit_web_view_get_uri(view), what);
	gtk_dialog_add_buttons(GTK_DIALOG(dialog), "Deny", GTK_RESPONSE_CANCEL, "Allow", GTK_RESPONSE_ACCEPT, NULL);
	gtk_dialog_set_default_response(GTK_DIALOG(dialog), GTK_RESPONSE_CANCEL);
	g_object_ref(request);
	g_signal_connect(dialog, "response", G_CALLBACK(permission_response), request);
	g_signal_connect(dialog, "destroy", G_CALLBACK(permission_destroy), request);
	gtk_widget_show(dialog);
	return TRUE;
}

static void
download_response(GtkNativeDialog *dialog, int response, WebKitDownload *download)
{
	if (response == GTK_RESPONSE_ACCEPT) {
		char *uri = gtk_file_chooser_get_uri(GTK_FILE_CHOOSER(dialog));
		webkit_download_set_allow_overwrite(download, TRUE);
		webkit_download_set_destination(download, uri);
		g_free(uri);
	} else webkit_download_cancel(download);
	g_object_unref(download);
	g_object_unref(dialog);
}

static gboolean
download_destination(WebKitDownload *download, const char *suggestion, gpointer data)
{
	(void)data;
	WebKitWebView *view = webkit_download_get_web_view(download);
	GtkWidget *window = view ? gtk_widget_get_toplevel(GTK_WIDGET(view)) : NULL;
	GtkFileChooserNative *dialog = gtk_file_chooser_native_new("Save download",
	    GTK_IS_WINDOW(window) ? GTK_WINDOW(window) : NULL, GTK_FILE_CHOOSER_ACTION_SAVE, "Save", "Cancel");
	char *basename = g_path_get_basename(suggestion);
	gtk_file_chooser_set_current_name(GTK_FILE_CHOOSER(dialog), basename);
	g_free(basename);
	gtk_file_chooser_set_do_overwrite_confirmation(GTK_FILE_CHOOSER(dialog), TRUE);
	g_signal_connect(dialog, "response", G_CALLBACK(download_response), g_object_ref(download));
	gtk_native_dialog_show(GTK_NATIVE_DIALOG(dialog));
	return TRUE;
}

static void
download_failed(WebKitDownload *download, GError *error, gpointer data)
{
	(void)download; (void)data;
	if (!g_error_matches(error, WEBKIT_DOWNLOAD_ERROR, WEBKIT_DOWNLOAD_ERROR_CANCELLED_BY_USER))
		g_warning("Download failed: %s", error->message);
}

static void
download_started(WebKitWebContext *ctx, WebKitDownload *download, gpointer data)
{
	(void)ctx; (void)data;
	g_signal_connect(download, "decide-destination", G_CALLBACK(download_destination), NULL);
	g_signal_connect(download, "failed", G_CALLBACK(download_failed), NULL);
}

static void
explorer_request(WebKitURISchemeRequest *request, gpointer data)
{
	(void)data;
	if (strcmp(webkit_uri_scheme_request_get_uri(request), WORKSPACE_BROWSER_HOME)) {
		GError *error = g_error_new_literal(G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "Unknown Worminal page");
		webkit_uri_scheme_request_finish_error(request, error);
		g_error_free(error);
		return;
	}
	/* Serve the built artifact locally on every viewing machine; saved links must
	 * not depend on a build checkout or a remote machine's installation prefix. */
	GInputStream *stream = g_memory_input_stream_new_from_data(file_explorer_html,
	                                                        sizeof(file_explorer_html) - 1, NULL);
	webkit_uri_scheme_request_finish(request, stream, sizeof(file_explorer_html) - 1, "text/html; charset=utf-8");
	g_object_unref(stream);
}

static WebKitWebContext *
browser_context(void)
{
	if (context) return context;
	char *data = g_build_filename(g_get_user_data_dir(), "worminal-preview", NULL);
	char *cache = g_build_filename(g_get_user_cache_dir(), "worminal-preview", NULL);
	g_mkdir_with_parents(data, 0700);
	g_mkdir_with_parents(cache, 0700);
	WebKitWebsiteDataManager *manager = webkit_website_data_manager_new(
	    "base-data-directory", data, "base-cache-directory", cache, NULL);
	webkit_website_data_manager_set_tls_errors_policy(manager, WEBKIT_TLS_ERRORS_POLICY_FAIL);
	context = webkit_web_context_new_with_website_data_manager(manager);
	webkit_web_context_register_uri_scheme(context, WORKSPACE_BROWSER_SCHEME, explorer_request, NULL, NULL);
	g_object_unref(manager);
	char *cookies = g_build_filename(data, "cookies.sqlite", NULL);
	WebKitCookieManager *cookie_manager = webkit_web_context_get_cookie_manager(context);
	webkit_cookie_manager_set_persistent_storage(cookie_manager, cookies, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
	webkit_cookie_manager_set_accept_policy(cookie_manager, WEBKIT_COOKIE_POLICY_ACCEPT_NO_THIRD_PARTY);
	webkit_web_context_set_spell_checking_enabled(context, FALSE);
	g_signal_connect(context, "download-started", G_CALLBACK(download_started), NULL);
	g_free(data); g_free(cache); g_free(cookies);
	return context;
}

Browser *
browser_new(App *app)
{
	Browser *b = g_new0(Browser, 1);
	b->app = app;
	b->box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
	g_object_ref_sink(b->box);
	b->entry = gtk_entry_new();
	gtk_entry_set_placeholder_text(GTK_ENTRY(b->entry), "URL");
	/* Tab activation/reparenting uses show_all. Only Ctrl+K reveals this entry. */
	gtk_widget_set_no_show_all(b->entry, TRUE);
	gtk_box_pack_start(GTK_BOX(b->box), b->entry, FALSE, FALSE, 0);
	b->notice = gtk_label_new("");
	gtk_label_set_line_wrap(GTK_LABEL(b->notice), TRUE);
	gtk_widget_set_no_show_all(b->notice, TRUE);
	gtk_box_pack_start(GTK_BOX(b->box), b->notice, FALSE, FALSE, 4);
	b->view = WEBKIT_WEB_VIEW(webkit_web_view_new_with_context(browser_context()));
	/* Adapt surf's conservative defaults; keep WebKit's normal UA/site support.
	 * No standalone surf configuration, scripts, or X-property command transport. */
	WebKitSettings *settings = webkit_settings_new_with_settings(
	    "enable-javascript", TRUE, "auto-load-images", TRUE, "default-charset", "UTF-8",
	    "enable-developer-extras", FALSE, "enable-dns-prefetching", FALSE,
	    "media-playback-requires-user-gesture", TRUE, "enable-smooth-scrolling", FALSE,
	    "enable-webgl", FALSE, "javascript-can-open-windows-automatically", FALSE, NULL);
	webkit_web_view_set_settings(b->view, settings);
	g_object_unref(settings);
	gtk_box_pack_start(GTK_BOX(b->box), GTK_WIDGET(b->view), TRUE, TRUE, 0);
	g_signal_connect(b->entry, "activate", G_CALLBACK(location), b);
	g_signal_connect(b->view, "notify::title", G_CALLBACK(changed), b);
	g_signal_connect(b->view, "notify::uri", G_CALLBACK(changed), b);
	g_signal_connect(b->view, "notify::is-loading", G_CALLBACK(changed), b);
	g_signal_connect(b->view, "load-changed", G_CALLBACK(load_changed), b);
	g_signal_connect(b->view, "load-failed", G_CALLBACK(load_failed), b);
	g_signal_connect(b->view, "load-failed-with-tls-errors", G_CALLBACK(tls_failed), b);
	g_signal_connect(b->view, "web-process-terminated", G_CALLBACK(renderer_failed), b);
	g_signal_connect(b->view, "enter-fullscreen", G_CALLBACK(fullscreen), b);
	g_signal_connect(b->view, "permission-request", G_CALLBACK(permission), b);
	g_signal_connect(b->view, "decide-policy", G_CALLBACK(policy), b);
	g_signal_connect(b->view, "create", G_CALLBACK(popup), b);
	g_signal_connect(b->view, "close", G_CALLBACK(close_requested), b);
	return b;
}

GtkWidget *browser_widget(Browser *b) { return b->box; }
GtkWidget *browser_view(Browser *b) { return GTK_WIDGET(b->view); }
static void browser_location(Browser *b)
{
	const char *url = webkit_web_view_get_uri(b->view);
	gtk_entry_set_text(GTK_ENTRY(b->entry), url && strcmp(url, "about:blank") ? url : "");
	gtk_widget_show(b->entry);
	gtk_widget_grab_focus(b->entry);
	gtk_editable_select_region(GTK_EDITABLE(b->entry), 0, -1);
}
gboolean
browser_shortcut(Page *page, guint key, guint mods)
{
	gboolean location_key = mods == GDK_CONTROL_MASK && key == GDK_KEY_k;
	gboolean history_key = mods == GDK_MOD1_MASK && (key == GDK_KEY_Left || key == GDK_KEY_Right);
	gboolean reload_key = (mods == GDK_CONTROL_MASK && key == GDK_KEY_r) || (!mods && key == GDK_KEY_F5);
	gboolean escape = !mods && key == GDK_KEY_Escape && page->browser &&
	    (gtk_widget_get_visible(page->browser->entry) || webkit_web_view_is_loading(page->browser->view));
	if (!location_key && !history_key && !reload_key && !escape) return FALSE;
	app_select(page);
	Browser *b = page->browser;
	if (location_key) browser_location(b);
	else if (history_key) {
		if (key == GDK_KEY_Left) webkit_web_view_go_back(b->view);
		else webkit_web_view_go_forward(b->view);
	} else if (reload_key) {
		notice(b, NULL);
		webkit_web_view_reload(b->view);
	} else {
		if (gtk_widget_get_visible(b->entry)) gtk_widget_hide(b->entry);
		else webkit_web_view_stop_loading(b->view);
		gtk_widget_grab_focus(GTK_WIDGET(b->view));
	}
	return TRUE;
}

void browser_zoom(Browser *b, double zoom) { webkit_web_view_set_zoom_level(b->view, MAX(.25, 1 + zoom / 10)); }
void browser_bind(Browser *b, Page *page, const char *url)
{
	b->page = page;
	b->app = page->app;
	browser_zoom(b, b->app->zoom);
	if (!webkit_web_view_get_uri(b->view)) webkit_web_view_load_uri(b->view, url);
	changed(b->view, NULL, b);
}
void browser_free(Browser *b)
{
	g_signal_handlers_disconnect_by_data(b->view, b);
	if (!b->app->closing) gtk_widget_destroy(b->box);
	g_object_unref(b->box);
	g_free(b);
}

G_MODULE_EXPORT const BrowserModule *
worminal_browser_module(unsigned int abi)
{
	static const BrowserModule module = {
		browser_new, browser_widget, browser_view, browser_bind,
		browser_shortcut, browser_zoom, browser_free
	};
	return abi == WORMINAL_BROWSER_ABI ? &module : NULL;
}
