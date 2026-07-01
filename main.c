#include <gtk/gtk.h>
#include <webkit/webkit.h>

static void
on_activate(GtkApplication *app, gpointer user_data)
{
    GtkWidget *window;
    GtkWidget *web_view;

    (void)user_data;

    window = gtk_application_window_new(app);
    gtk_window_set_default_size(GTK_WINDOW(window), 1200, 800);
    gtk_window_set_title(GTK_WINDOW(window), "ubar");

    web_view = webkit_web_view_new();
    gtk_window_set_child(GTK_WINDOW(window), web_view);
    webkit_web_view_load_uri(WEBKIT_WEB_VIEW(web_view), "https://example.com");

    gtk_window_present(GTK_WINDOW(window));
}

int
main(int argc, char *argv[])
{
    GtkApplication *app;
    int status;

    app = gtk_application_new("dev.ghanti.ubar", G_APPLICATION_DEFAULT_FLAGS);
    g_signal_connect(app, "activate", G_CALLBACK(on_activate), NULL);

    status = g_application_run(G_APPLICATION(app), argc, argv);
    g_object_unref(app);

    return status;
}
