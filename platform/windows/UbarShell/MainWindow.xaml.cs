using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace UbarShell;

public sealed partial class MainWindow : Window
{
    private sealed class TabState
    {
        public required NativeViewHost Host { get; init; }
        public ulong View { get; set; }
        public double Zoom { get; set; } = 1;
    }

    private readonly EngineBridge engine = EngineBridge.Load();
    private readonly bool privateMode;
    private readonly DispatcherQueueTimer memoryTimer;

    public MainWindow()
    {
        InitializeComponent();
        SystemBackdrop = new MicaBackdrop();
        ExtendsContentIntoTitleBar = true;
        privateMode = Environment.GetCommandLineArgs().Contains("--incognito");
        if (privateMode) PrivateButton.IsEnabled = false;
        engine.Attach(privateMode);
        engine.EventReceived += Engine_EventReceived;
        memoryTimer = DispatcherQueue.CreateTimer();
        memoryTimer.Interval = TimeSpan.FromSeconds(5);
        memoryTimer.Tick += (_, _) => engine.ReportMemory();
        memoryTimer.Start();
        RenderExtensionActions();
        AddTab("New Tab");
        Title = $"{(privateMode ? "uBar Private" : "uBar")} — {engine.Status}";
        Closed += (_, _) => {
            memoryTimer.Stop();
            CloseTabs();
        };
    }

    private void AddTab(string title)
    {
        var host = new NativeViewHost(this);
        var state = new TabState { Host = host };
        var item = new TabViewItem {
            Header = title, IconSource = new SymbolIconSource { Symbol = Symbol.World },
            Content = host, IsClosable = true, Tag = state,
        };
        host.HandleReady += ready => state.View = engine.CreateView(
            ready.Handle,
            (uint)Math.Max(1, ready.ActualWidth * ready.XamlRoot.RasterizationScale),
            (uint)Math.Max(1, ready.ActualHeight * ready.XamlRoot.RasterizationScale),
            ready.XamlRoot.RasterizationScale);
        Tabs.TabItems.Add(item);
        Tabs.SelectedItem = item;
    }

    private void CloseTab(TabViewItem item)
    {
        if (item.Tag is not TabState state) return;
        engine.DestroyViewHandle(state.View);
        state.Host.Dispose();
    }

    private void CloseTabs()
    {
        foreach (TabViewItem item in Tabs.TabItems) CloseTab(item);
        engine.Dispose();
    }

    private void Tabs_AddTabButtonClick(TabView sender, object args) => AddTab("New Tab");

    private void Tabs_TabCloseRequested(TabView sender, TabViewTabCloseRequestedEventArgs args)
    {
        CloseTab(args.Tab);
        sender.TabItems.Remove(args.Tab);
        if (sender.TabItems.Count == 0) Close();
    }

    private void Tabs_SelectionChanged(object sender, SelectionChangedEventArgs args)
    {
        foreach (TabViewItem item in Tabs.TabItems) {
            if (item.Tag is not TabState state) continue;
            bool selected = ReferenceEquals(item, Tabs.SelectedItem);
            state.Host.SetNativeVisible(selected);
            engine.SetViewVisible(state.View, selected);
        }
    }

    private void Address_QuerySubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        if (Tabs.SelectedItem is TabViewItem { Tag: TabState state })
            engine.Navigate(state.View, args.QueryText);
    }

    private ulong SelectedView()
        => Tabs.SelectedItem is TabViewItem { Tag: TabState state } ? state.View : 0;

    private void Back_Click(object sender, RoutedEventArgs args) => engine.GoBack(SelectedView());
    private void Forward_Click(object sender, RoutedEventArgs args) => engine.GoForward(SelectedView());
    private void Reload_Click(object sender, RoutedEventArgs args) => engine.Reload(SelectedView());

    private void ZoomOut_Click(object sender, RoutedEventArgs args) => ChangeZoom(-.1);
    private void ZoomIn_Click(object sender, RoutedEventArgs args) => ChangeZoom(.1);

    private void ChangeZoom(double delta)
    {
        if (Tabs.SelectedItem is not TabViewItem { Tag: TabState state }) return;
        state.Zoom = Math.Clamp(Math.Round((state.Zoom + delta) * 10) / 10, .5, 5);
        engine.SetZoom(state.View, state.Zoom);
    }

    private void Private_Click(object sender, RoutedEventArgs args)
        => System.Diagnostics.Process.Start(Environment.ProcessPath!, "--incognito");

    private async void Bookmark_Click(object sender, RoutedEventArgs args)
    {
        string url = Address.Text?.Trim() ?? "";
        if (privateMode || !Uri.TryCreate(url, UriKind.Absolute, out _)) return;
        string title = FindTab(SelectedView())?.Header?.ToString() ?? url;
        if (!engine.AddBookmark(title, url)) return;
        var dialog = new ContentDialog { XamlRoot = Tabs.XamlRoot, Title = "Bookmarked",
            Content = title, CloseButtonText = "Close" };
        await dialog.ShowAsync();
    }

    private async void Library_Click(object sender, RoutedEventArgs args)
    {
        var dialog = new ContentDialog { XamlRoot = Tabs.XamlRoot, Title = "Library",
            Content = new ScrollViewer { Content = new TextBlock {
                Text = engine.LibrarySummary(), IsTextSelectionEnabled = true, TextWrapping = TextWrapping.Wrap,
            }, MaxHeight = 520 }, CloseButtonText = "Close" };
        await dialog.ShowAsync();
    }

    private void RenderExtensionActions()
    {
        ExtensionActions.Children.Clear();
        foreach (var action in engine.GetExtensionActions()) {
            var button = new Button {
                Content = action.Title, Tag = action.Id,
            };
            AutomationProperties.SetName(button, action.Title);
            button.Click += Extension_Click;
            ExtensionActions.Children.Add(button);
        }
    }

    private async void InstallExtension_Click(object sender, RoutedEventArgs args)
    {
        var picker = new Windows.Storage.Pickers.FileOpenPicker();
        picker.FileTypeFilter.Add(".xpi");
        picker.FileTypeFilter.Add(".crx");
        WinRT.Interop.InitializeWithWindow.Initialize(
            picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        var file = await picker.PickSingleFileAsync();
        if (file is null) return;
        if (engine.InstallExtension(file.Path)) {
            RenderExtensionActions();
            return;
        }
        var dialog = new ContentDialog {
            XamlRoot = ExtensionActions.XamlRoot,
            Title = "Extension rejected",
            Content = "Package signature, integrity, or compatibility validation failed.",
            CloseButtonText = "Close",
        };
        await dialog.ShowAsync();
    }

    private async void Extension_Click(object sender, RoutedEventArgs args)
    {
        if (sender is not Button { Tag: string id } button) return;
        string? popup = engine.GetExtensionPopup(id);
        ulong view = SelectedView();
        if (popup is not null && view != 0) {
            Address.Text = popup;
            engine.Navigate(view, popup);
            return;
        }
        var dialog = new ContentDialog {
            XamlRoot = ExtensionActions.XamlRoot,
            Title = button.Content?.ToString() ?? "Extension",
            Content = "Extension action has no popup.",
            CloseButtonText = "Close",
        };
        await dialog.ShowAsync();
    }

    private void Engine_EventReceived(EngineBridge.EngineEvent engineEvent)
    {
        DispatcherQueue.TryEnqueue(() => {
            if (engineEvent.Kind == 3 && FindTab(engineEvent.View) is { } titled && engineEvent.Text.Length != 0)
                titled.Header = engineEvent.Text;
            else if (engineEvent.Kind == 4 && engineEvent.View == SelectedView())
                Address.Text = engineEvent.Text;
            else if (engineEvent.Kind == 5 && FindTab(engineEvent.View) is { } crashed)
                crashed.Header = "Crashed";
            else if (engineEvent.Kind == 7 && File.Exists(engineEvent.Text)) {
                if (privateMode) CleanupDownloadedExtension(engineEvent.Text);
                else ConfirmDownloadedExtension(engineEvent.Text);
            }
        });
    }

    private async void ConfirmDownloadedExtension(string path)
    {
        var dialog = new ContentDialog {
            XamlRoot = ExtensionActions.XamlRoot,
            Title = "Install extension?",
            Content = "Only store-signed Firefox or Chrome packages pass verification.",
            PrimaryButtonText = "Install",
            CloseButtonText = "Cancel",
        };
        if (await dialog.ShowAsync() == ContentDialogResult.Primary && engine.InstallExtension(path))
            RenderExtensionActions();
        CleanupDownloadedExtension(path);
    }

    private static void CleanupDownloadedExtension(string path)
    {
        try { Directory.Delete(Path.GetDirectoryName(path)!, true); }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException) { }
    }

    private TabViewItem? FindTab(ulong view)
        => Tabs.TabItems.OfType<TabViewItem>()
            .FirstOrDefault(item => item.Tag is TabState state && state.View == view);
}
