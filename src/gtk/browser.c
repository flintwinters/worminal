/* Internal WebKit views move between local windows; profiles stay on this machine. */
#define _GNU_SOURCE
#include <webkit2/webkit2.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <gio/gunixinputstream.h>
#include "preview.h"
#include "browser_module.h"
#include "browser_target.h"
#include "build/browser_resources.h"


struct Browser {
	App *app;
	Page *page;
	GtkWidget *box, *entry, *find, *notice;
	WebKitWebView *view;
	gboolean select_find;
};
static const BrowserHost *host;
static WebKitWebContext *context;
static GList *browsers;
static char *local_document(const char *);

static WebKitWebView *
related_view(const char *url)
{
	WebKitSecurityOrigin *origin = webkit_security_origin_new_for_uri(url);
	const char *scheme = webkit_security_origin_get_protocol(origin);
	WebKitWebView *result = NULL;
	/* A file tab may reuse only the same local path, never another file's
	 * renderer. Network tabs share only matching WebKit security origins. */
	char *document = local_document(url);
	GFile *file = document ? g_file_new_for_uri(document) : NULL;
	g_free(document);
	if (file || !g_strcmp0(scheme, "http") || !g_strcmp0(scheme, "https")) {
		for (GList *item = browsers; item; item = item->next) {
			Browser *b = item->data;
			const char *current = webkit_web_view_get_uri(b->view);
			if (!current) continue;
			if (file) {
				char *other_document = local_document(current);
				if (!other_document) continue;
				GFile *other = g_file_new_for_uri(other_document);
				g_free(other_document);
				gboolean same = g_file_equal(file, other);
				g_object_unref(other);
				if (same) { result = b->view; break; }
				continue;
			}
			WebKitSecurityOrigin *other = webkit_security_origin_new_for_uri(current);
			gboolean same = !g_strcmp0(scheme, webkit_security_origin_get_protocol(other)) &&
			    !g_strcmp0(webkit_security_origin_get_host(origin), webkit_security_origin_get_host(other)) &&
			    webkit_security_origin_get_port(origin) == webkit_security_origin_get_port(other);
			webkit_security_origin_unref(other);
			if (same) { result = b->view; break; }
		}
	}
	g_clear_object(&file);
	webkit_security_origin_unref(origin);
	return result;
}

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

static gboolean
find_visible(Browser *b)
{
	return b->find && gtk_widget_get_visible(b->find);
}

static void
find_present(Browser *b, const char *action)
{
	char *script = g_strconcat(browser_find_script, "\nWorminalFind.update('", action, "');", NULL);
	webkit_web_view_evaluate_javascript(b->view, script, -1, "worminal-find", NULL, NULL, NULL, NULL);
	g_free(script);
}

static void
find_completed(Browser *b)
{
	/* After navigation, WebKit can claim the primary selection while searching.
	 * Reapply Ctrl+F's selection on completion, unless editing or closing superseded it. */
	if (b->select_find && gtk_widget_is_focus(b->find))
		gtk_editable_select_region(GTK_EDITABLE(b->find), 0, -1);
	b->select_find = FALSE;
}

static void
find_matched(WebKitFindController *controller, guint matches, Browser *b)
{
	(void)controller; (void)matches;
	if (find_visible(b)) find_present(b, "show");
	find_completed(b);
}

static void
find_close(Browser *b)
{
	if (!b->find) return;
	b->select_find = FALSE;
	gtk_widget_hide(b->find);
	find_present(b, "clear");
	/* WebKit keeps match highlights until the owner explicitly finishes searching. */
	webkit_find_controller_search_finish(webkit_web_view_get_find_controller(b->view));
}

static void
find_changed(GtkEntry *entry, Browser *b)
{
	b->select_find = FALSE;
	find_present(b, "clear");
	GtkStyleContext *style = gtk_widget_get_style_context(b->find);
	gtk_style_context_remove_class(style, "error");
	gtk_widget_set_tooltip_text(b->find, "Enter: next match; Shift+Enter: previous; Escape: select result");
	WebKitFindController *controller = webkit_web_view_get_find_controller(b->view);
	const char *text = gtk_entry_get_text(entry);
	if (!find_visible(b) || !*text) webkit_find_controller_search_finish(controller);
	else webkit_find_controller_search(controller, text,
	    WEBKIT_FIND_OPTIONS_CASE_INSENSITIVE | WEBKIT_FIND_OPTIONS_WRAP_AROUND, G_MAXUINT);
}

static void
find_failed(WebKitFindController *controller, Browser *b)
{
	(void)controller;
	find_completed(b);
	if (!find_visible(b) || !*gtk_entry_get_text(GTK_ENTRY(b->find))) return;
	find_present(b, "clear");
	gtk_style_context_add_class(gtk_widget_get_style_context(b->find), "error");
	gtk_widget_set_tooltip_text(b->find, "No matches");
}

static void
browser_find(Browser *b)
{
	if (!b->find) {
		b->find = gtk_search_entry_new();
		gtk_entry_set_placeholder_text(GTK_ENTRY(b->find), "Find in page");
		/* As with the URL entry, moving a live view must not reveal a closed search. */
		gtk_widget_set_no_show_all(b->find, TRUE);
		gtk_box_pack_start(GTK_BOX(b->box), b->find, FALSE, FALSE, 0);
		gtk_box_reorder_child(GTK_BOX(b->box), b->find, 0);
		g_signal_connect(b->find, "changed", G_CALLBACK(find_changed), b);
		g_signal_connect(webkit_web_view_get_find_controller(b->view),
		    "failed-to-find-text", G_CALLBACK(find_failed), b);
		g_signal_connect(webkit_web_view_get_find_controller(b->view),
		    "found-text", G_CALLBACK(find_matched), b);
	}
	if (b->entry) gtk_widget_hide(b->entry);
	gtk_widget_show(b->find);
	/* Starting a search can return focus from an accepted link to WebKit.
	 * Establish the entry selection after that focus transition. */
	find_changed(GTK_ENTRY(b->find), b);
	b->select_find = *gtk_entry_get_text(GTK_ENTRY(b->find)) != '\0';
	gtk_widget_grab_focus(b->find);
	gtk_editable_select_region(GTK_EDITABLE(b->find), 0, -1);
}

static void
changed(WebKitWebView *view, GParamSpec *property, Browser *b)
{
	const char *url = webkit_web_view_get_uri(view);
	const char *title = webkit_web_view_get_title(view);
	if (b->entry && url && !gtk_widget_has_focus(b->entry))
		gtk_entry_set_text(GTK_ENTRY(b->entry), url);
	if (b->page) {
		host->title(b->page, title && *title ? title : url ? url : "Browser");
		if (property && !strcmp(property->name, "uri")) host->navigate(b->page, url);
	}
}

static void
load_changed(WebKitWebView *view, WebKitLoadEvent event, Browser *b)
{
	if (event == WEBKIT_LOAD_STARTED) {
		host->timing("browser-load-start");
	}
	if (event == WEBKIT_LOAD_COMMITTED) host->timing("browser-load-commit");
	if (event == WEBKIT_LOAD_FINISHED) host->timing("browser-load-finish");
	if (event == WEBKIT_LOAD_STARTED) { notice(b, NULL); find_close(b); }
	changed(view, NULL, b);
}

static void
error_page(WebKitWebView *view, const char *url, const char *message)
{
	char *escaped = g_markup_escape_text(message, -1);
	char **parts = g_strsplit(browser_error_html, "{{message}}", 2);
	char *html = g_strjoinv(escaped, parts);
	g_strfreev(parts);
	webkit_web_view_load_alternate_html(view, html, url, NULL);
	g_free(html); g_free(escaped);
}

static gboolean
load_failed(WebKitWebView *view, WebKitLoadEvent event, const char *url, GError *error, Browser *b)
{
	(void)event; (void)b;
	/* Downloads and media documents hand navigation to another loader. WebKit's
	 * WILL_HANDLE_LOAD is preliminary, not a failure; alternate HTML aborts the player. */
	if (!g_error_matches(error, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED) &&
	    !g_error_matches(error, WEBKIT_POLICY_ERROR, WEBKIT_POLICY_ERROR_FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE) &&
	    !g_error_matches(error, WEBKIT_PLUGIN_ERROR, WEBKIT_PLUGIN_ERROR_WILL_HANDLE_LOAD))
		error_page(view, url, error->message);
	return TRUE;
}

static gboolean
tls_failed(WebKitWebView *view, const char *url, GTlsCertificate *certificate,
           GTlsCertificateFlags errors, Browser *b)
{
	(void)certificate; (void)b;
	char *message = g_strdup_printf("Certificate validation failed (%u) for %s. Connection blocked.", errors, url);
	error_page(view, url, message);
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
		host->open(b->app, webkit_uri_request_get_uri(webkit_navigation_action_get_request(action)));
	return NULL;
}

static void
close_requested(WebKitWebView *view, Browser *b)
{
	(void)view;
	if (b->page) host->close(b->page);
}

static gboolean
policy(WebKitWebView *view, WebKitPolicyDecision *decision, WebKitPolicyDecisionType type, Browser *b)
{
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
			if (webkit_navigation_action_is_user_gesture(action)) host->open(b->app, url);
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

static char *
local_document(const char *url)
{
	GUri *uri = url ? g_uri_parse(url, G_URI_FLAGS_ENCODED, NULL) : NULL;
	if (!uri) return NULL;
	char *document = !g_strcmp0(g_uri_get_scheme(uri), "file") &&
	    (!g_uri_get_host(uri) || !*g_uri_get_host(uri) || !g_ascii_strcasecmp(g_uri_get_host(uri), "localhost")) ?
	    g_uri_to_string_partial(uri, G_URI_HIDE_QUERY | G_URI_HIDE_FRAGMENT) : NULL;
	g_uri_unref(uri);
	return document;
}

static void
local_response(WebKitURISchemeRequest *request, GInputStream *stream, gint64 length, const char *type)
{
	WebKitURISchemeResponse *response = webkit_uri_scheme_response_new(stream, length);
	webkit_uri_scheme_response_set_content_type(response, type);
	SoupMessageHeaders *headers = soup_message_headers_new(SOUP_MESSAGE_HEADERS_RESPONSE);
	soup_message_headers_append(headers, "Access-Control-Allow-Origin", "null");
	/* WebKitURISchemeResponse takes full ownership of the Soup headers. */
	webkit_uri_scheme_response_set_http_headers(response, headers);
	webkit_uri_scheme_request_finish_with_response(request, response);
	g_object_unref(response);
	g_object_unref(stream);
}

/* Local HTML explicitly selected by the user may read metadata and bounded files.
 * Navigating away never grants the next document that capability. */
static void
local_request(WebKitURISchemeRequest *request, gpointer data)
{
	(void)data;
	GError *error = NULL;
	WebKitWebView *view = webkit_uri_scheme_request_get_web_view(request);
	const char *trusted = view ? g_object_get_data(G_OBJECT(view), "worminal-local-document") : NULL;
	char *top = view ? local_document(webkit_web_view_get_uri(view)) : NULL;
	gboolean allowed = trusted && !g_strcmp0(top, trusted);
	g_free(top);
	GUri *uri = g_uri_parse(webkit_uri_scheme_request_get_uri(request), G_URI_FLAGS_ENCODED_QUERY, &error);
	if (!uri) goto fail;
	if (!allowed || (g_uri_get_host(uri) && *g_uri_get_host(uri)) ||
		(strcmp(g_uri_get_path(uri), "/list") && strcmp(g_uri_get_path(uri), "/file"))) {
		g_uri_unref(uri);
		goto denied;
	}
	gboolean read_file = !strcmp(g_uri_get_path(uri), "/file");
	/* URLSearchParams uses form encoding: decode spaces while preserving literal %2B. */
	GHashTable *params = g_uri_parse_params(g_uri_get_query(uri) ? g_uri_get_query(uri) : "",
	    -1, "&", G_URI_PARAMS_WWW_FORM, &error);
	g_uri_unref(uri);
	if (!params) goto fail;
	const char *credential = g_object_get_data(G_OBJECT(view), "worminal-local-credential");
	if (!credential || g_strcmp0(credential, g_hash_table_lookup(params, "credential"))) {
		g_hash_table_unref(params);
		goto denied;
	}
	const char *path = g_hash_table_lookup(params, "path");
	if (!path || !g_path_is_absolute(path)) {
		g_hash_table_unref(params);
		error = g_error_new_literal(G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT, "Local path must be absolute");
		goto fail;
	}
	if (read_file) {
		/* Open nonblocking before fstat: a replacement FIFO must never stall GTK.
		 * Bound data and reject devices on the descriptor actually returned, not a prior stat. */
		int fd = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC | O_NOCTTY);
		struct stat info;
		g_hash_table_unref(params);
		if (fd < 0 || fstat(fd, &info) || !S_ISREG(info.st_mode) ||
		    info.st_size < 0 || info.st_size > 32 * 1024 * 1024) {
			if (fd >= 0) close(fd);
			error = g_error_new_literal(G_IO_ERROR, G_IO_ERROR_FAILED,
			    "Preview requires a regular file no larger than 32 MiB");
			goto fail;
		}
		GInputStream *stream = g_unix_input_stream_new(fd, TRUE);
		local_response(request, stream, info.st_size, "application/octet-stream");
		return;
	}
	GFile *file = g_file_new_for_path(path);
	GFileEnumerator *items = g_file_enumerate_children(file,
		"standard::name,standard::type,standard::size,time::modified,time::modified-usec",
		G_FILE_QUERY_INFO_NONE, NULL, &error);
	g_object_unref(file);
	g_hash_table_unref(params);
	if (!items) goto fail;
	GString *json = g_string_new("[");
	GFileInfo *info;
	while ((info = g_file_enumerator_next_file(items, NULL, &error))) {
		char *encoded = g_base64_encode((const guchar *)g_file_info_get_name(info),
		    strlen(g_file_info_get_name(info)));
		g_string_append_printf(json, "%s[\"%s\",%s,%" G_GOFFSET_FORMAT ",%" G_GUINT64_FORMAT "]",
			json->len > 1 ? "," : "", encoded,
			g_file_info_get_file_type(info) == G_FILE_TYPE_DIRECTORY ? "true" : "false",
			g_file_info_get_size(info),
			g_file_info_get_attribute_uint64(info, G_FILE_ATTRIBUTE_TIME_MODIFIED) * 1000 +
			g_file_info_get_attribute_uint32(info, G_FILE_ATTRIBUTE_TIME_MODIFIED_USEC) / 1000);
		g_free(encoded);
		g_object_unref(info);
	}
	g_object_unref(items);
	if (error) { g_string_free(json, TRUE); goto fail; }
	g_string_append_c(json, ']');
	gsize length = json->len;
	GInputStream *stream = g_memory_input_stream_new_from_data(g_string_free(json, FALSE), length, g_free);
	local_response(request, stream, length, "application/json");
	return;
 denied:
	error = g_error_new_literal(G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
	    "Local access requires the selected local document");
 fail:
	webkit_uri_scheme_request_finish_error(request, error);
	g_error_free(error);
}

static WebKitWebContext *
browser_context(void)
{
	if (context) return context;
	host->timing("browser-context-start");
	char *data = g_build_filename(g_get_user_data_dir(), "worminal-preview", NULL);
	char *cache = g_build_filename(g_get_user_cache_dir(), "worminal-preview", NULL);
	g_mkdir_with_parents(data, 0700);
	g_mkdir_with_parents(cache, 0700);
	WebKitWebsiteDataManager *manager = webkit_website_data_manager_new(
	    "base-data-directory", data, "base-cache-directory", cache, NULL);
	webkit_website_data_manager_set_tls_errors_policy(manager, WEBKIT_TLS_ERRORS_POLICY_FAIL);
	context = webkit_web_context_new_with_website_data_manager(manager);
	webkit_web_context_register_uri_scheme(context, WORKSPACE_BROWSER_SCHEME, local_request, NULL, NULL);
	webkit_security_manager_register_uri_scheme_as_cors_enabled(webkit_web_context_get_security_manager(context),
	                                                     WORKSPACE_BROWSER_SCHEME);
	g_object_unref(manager);
	char *cookies = g_build_filename(data, "cookies.sqlite", NULL);
	WebKitCookieManager *cookie_manager = webkit_web_context_get_cookie_manager(context);
	webkit_cookie_manager_set_persistent_storage(cookie_manager, cookies, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
	webkit_cookie_manager_set_accept_policy(cookie_manager, WEBKIT_COOKIE_POLICY_ACCEPT_NO_THIRD_PARTY);
	webkit_web_context_set_spell_checking_enabled(context, FALSE);
	g_signal_connect(context, "download-started", G_CALLBACK(download_started), NULL);
	g_free(data); g_free(cache); g_free(cookies);
	host->timing("browser-context-ready");
	return context;
}

Browser *
browser_new(App *app, const char *url)
{
	Browser *b = g_new0(Browser, 1);
	b->app = app;
	b->box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
	g_object_ref_sink(b->box);
	b->notice = gtk_label_new("");
	gtk_label_set_line_wrap(GTK_LABEL(b->notice), TRUE);
	gtk_widget_set_no_show_all(b->notice, TRUE);
	gtk_box_pack_start(GTK_BOX(b->box), b->notice, FALSE, FALSE, 4);
	WebKitWebView *related = related_view(url);
	/* Each view owns its capability script, even when WebKit shares a renderer. */
	WebKitUserContentManager *manager = webkit_user_content_manager_new();
	b->view = WEBKIT_WEB_VIEW(related ?
	    g_object_new(WEBKIT_TYPE_WEB_VIEW, "related-view", related, "user-content-manager", manager, NULL) :
	    g_object_new(WEBKIT_TYPE_WEB_VIEW, "web-context", browser_context(), "user-content-manager", manager, NULL));
	char *document = local_document(url);
	if (document) {
		char *credential = g_uuid_string_random();
		char *escaped = g_strescape(document, NULL);
		char *source = g_strdup_printf(browser_local_script, escaped, credential);
		WebKitUserScript *script = webkit_user_script_new(source, WEBKIT_USER_CONTENT_INJECT_TOP_FRAME,
		    WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
		webkit_user_content_manager_add_script(manager, script);
		webkit_user_script_unref(script);
		g_free(source); g_free(escaped);
		g_object_set_data_full(G_OBJECT(b->view), "worminal-local-document", document, g_free);
		g_object_set_data_full(G_OBJECT(b->view), "worminal-local-credential", credential, g_free);
	}
	g_object_unref(manager);
	GdkRGBA background;
	gdk_rgba_parse(&background, "#181818");
	webkit_web_view_set_background_color(b->view, &background);
	browsers = g_list_prepend(browsers, b);
	host->timing("browser-view-ready");
	/* Adapt surf's conservative defaults; keep WebKit's normal UA/site support.
	 * No standalone surf configuration or X-property command transport. */
	WebKitSettings *settings = webkit_settings_new_with_settings(
	    "enable-javascript", TRUE, "auto-load-images", TRUE, "default-charset", "UTF-8",
	    "enable-developer-extras", FALSE, "enable-dns-prefetching", FALSE,
	    "media-playback-requires-user-gesture", TRUE, "enable-smooth-scrolling", FALSE,
	    "enable-webgl", FALSE, "javascript-can-open-windows-automatically", FALSE, NULL);
	webkit_web_view_set_settings(b->view, settings);
	g_object_unref(settings);
	gtk_box_pack_start(GTK_BOX(b->box), GTK_WIDGET(b->view), TRUE, TRUE, 0);
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
	find_close(b);
	if (!b->entry) {
		b->entry = gtk_entry_new();
		gtk_entry_set_placeholder_text(GTK_ENTRY(b->entry), "URL");
		/* Reparenting uses show_all; only Ctrl+K reveals the entry. */
		gtk_widget_set_no_show_all(b->entry, TRUE);
		gtk_box_pack_start(GTK_BOX(b->box), b->entry, FALSE, FALSE, 0);
		gtk_box_reorder_child(GTK_BOX(b->box), b->entry, 0);
		g_signal_connect(b->entry, "activate", G_CALLBACK(location), b);
	}
	const char *url = webkit_web_view_get_uri(b->view);
	gtk_entry_set_text(GTK_ENTRY(b->entry), url && strcmp(url, "about:blank") ? url : "");
	gtk_widget_show(b->entry);
	gtk_widget_grab_focus(b->entry);
	gtk_editable_select_region(GTK_EDITABLE(b->entry), 0, -1);
}

static gboolean
location_visible(Browser *b)
{
	return b->entry && gtk_widget_get_visible(b->entry);
}
gboolean
browser_shortcut(Page *page, guint key, guint mods)
{
	gboolean find_key = mods == GDK_CONTROL_MASK && key == GDK_KEY_f;
	gboolean find_step = page->browser && find_visible(page->browser) &&
	    gtk_widget_is_focus(page->browser->find) &&
	    (key == GDK_KEY_Return || key == GDK_KEY_KP_Enter) && (!mods || mods == GDK_SHIFT_MASK);
	gboolean location_key = mods == GDK_CONTROL_MASK && key == GDK_KEY_k;
	gboolean history_key = mods == GDK_MOD1_MASK && (key == GDK_KEY_Left || key == GDK_KEY_Right);
	gboolean reload_key = (mods == GDK_CONTROL_MASK && key == GDK_KEY_r) || (!mods && key == GDK_KEY_F5);
	gboolean escape = !mods && key == GDK_KEY_Escape && page->browser &&
	    (find_visible(page->browser) || location_visible(page->browser) ||
	     webkit_web_view_is_loading(page->browser->view));
	if (!find_key && !find_step && !location_key && !history_key && !reload_key && !escape) return FALSE;
	/* Selecting re-focuses the web view; match stepping must keep the find entry focused. */
	if (!find_step) host->select(page);
	Browser *b = page->browser;
	if (find_key) browser_find(b);
	else if (find_step) {
		if (*gtk_entry_get_text(GTK_ENTRY(b->find))) {
			WebKitFindController *controller = webkit_web_view_get_find_controller(b->view);
			if (mods == GDK_SHIFT_MASK) webkit_find_controller_search_previous(controller);
			else webkit_find_controller_search_next(controller);
		}
	} else if (location_key) browser_location(b);
	else if (history_key) {
		if (key == GDK_KEY_Left) webkit_web_view_go_back(b->view);
		else webkit_web_view_go_forward(b->view);
	} else if (reload_key) {
		notice(b, NULL);
		webkit_web_view_reload(b->view);
	} else {
		if (find_visible(b)) { find_close(b); find_present(b, "accept"); }
		else if (location_visible(b)) gtk_widget_hide(b->entry);
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
	webkit_web_view_set_background_color(b->view, &page->colors[259]);
	if (!webkit_web_view_get_uri(b->view)) webkit_web_view_load_uri(b->view, url);
	changed(b->view, NULL, b);
}
void browser_free(Browser *b)
{
	browsers = g_list_remove(browsers, b);
	g_signal_handlers_disconnect_by_data(webkit_web_view_get_find_controller(b->view), b);
	g_signal_handlers_disconnect_by_data(b->view, b);
	if (!b->app->closing) gtk_widget_destroy(b->box);
	g_object_unref(b->box);
	g_free(b);
}

static void browser_initialize(const BrowserHost *callbacks) { host = callbacks; }

G_MODULE_EXPORT const BrowserModule *
worminal_browser_module(unsigned int abi)
{
	static const BrowserModule module = {
		browser_new, browser_widget, browser_view, browser_bind,
		browser_shortcut, browser_zoom, browser_free, browser_initialize
	};
	return abi == WORMINAL_BROWSER_ABI ? &module : NULL;
}
