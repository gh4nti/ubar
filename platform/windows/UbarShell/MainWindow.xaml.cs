using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace UbarShell;

public sealed partial class MainWindow : Window
{
    private sealed class TabState
    {
        public required NativeViewHost Host { get; init; }
        public ulong View { get; set; }
    }

    private readonly EngineBridge engine = EngineBridge.Load();

    public MainWindow()
    {
        InitializeComponent();
        SystemBackdrop = new MicaBackdrop();
        ExtendsContentIntoTitleBar = true;
        bool privateMode = Environment.GetCommandLineArgs().Contains("--incognito");
        engine.Attach(privateMode);
        AddTab("New Tab");
        Title = $"{(privateMode ? "uBar Private" : "uBar")} — {engine.Status}";
        Closed += (_, _) => CloseTabs();
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

    private void Private_Click(object sender, RoutedEventArgs args)
        => System.Diagnostics.Process.Start(Environment.ProcessPath!, "--incognito");
}
