import AppKit
import ShelfScan
import ShelfScanCapture
import SwiftUI

struct ContentView: View {
    @ObservedObject var controller: DemoController
    @ObservedObject var model: ScanModel
    @State private var panel = Panel.items
    enum Panel { case items, settings }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ZStack(alignment: .topLeading) {
                    PreviewView(session: controller.session, frameSize: controller.frameSize, boxes: model.boxes)
                    if model.hiddenCount > 0 {
                        Text("\(model.hiddenCount) hidden").font(.caption).padding(4).background(.black.opacity(0.6)).foregroundColor(.white).padding(8)
                    }
                    if let hint = model.barcodeHint {
                        Text("Show the barcode of \(hint.itemId): \(hint.hint)").font(.callout).padding(6)
                            .background(.orange.opacity(0.85)).foregroundColor(.black).padding(.top, 32).padding(.leading, 8)
                    }
                }
                Divider()
                VStack(spacing: 0) {
                    Picker("", selection: $panel) {
                        Text("Items (\(controller.items.count))").tag(Panel.items)
                        Text(controller.draftChanged ? "Settings •" : "Settings").tag(Panel.settings)
                    }.pickerStyle(.segmented).labelsHidden().padding(8)
                    Divider()
                    switch panel {
                    case .items: ItemList(controller: controller)
                    case .settings: SettingsPanel(controller: controller)
                    }
                }.frame(width: 380)
            }
            Divider()
            StatsStrip(controller: controller)
        }
    }
}

struct ItemList: View {
    @ObservedObject var controller: DemoController
    @State private var editing: String?
    @State private var title = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Spacer()
                Button("Draft listings") { controller.draftListings() }.disabled(controller.engine == nil)
                Button("Export CSV") { controller.exportCSV() }.disabled(controller.engine == nil)
            }.padding(8)
            Divider()
            List(controller.items, id: \.id) { item in
                VStack(alignment: .leading, spacing: 4) {
                    HStack(alignment: .top) {
                        crop(item).frame(width: 64, height: 64)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.guess.title.isEmpty ? item.label : item.guess.title).font(.callout).lineLimit(2)
                            Text("\(item.id) · x\(item.quantity) · \(stateText(item.state))\(item.gtin.map { " · \($0)" } ?? "")")
                                .font(.caption).foregroundColor(.secondary)
                            if item.needsBarcode { Text("needs barcode").font(.caption2).foregroundColor(.orange) }
                        }
                    }
                    HStack(spacing: 4) {
                        Button("✓") { controller.confirm(item.id) }.help("Confirm")
                        Button("✗") { controller.reject(item.id, title: nil) }.help("Reject: try another guess")
                        Button("Title…") { editing = item.id; title = "" }
                        Button("Hide") { controller.hide(item.id) }.help("Exclude this and everything like it")
                        Button(controller.focused == item.id ? "Unfocus" : "Focus") { controller.focus(item.id) }
                    }.buttonStyle(.bordered).controlSize(.small)
                    if editing == item.id {
                        HStack {
                            TextField("Correct title", text: $title).onSubmit { submit(item.id) }
                            Button("OK") { submit(item.id) }
                        }
                    }
                }.padding(.vertical, 2)
            }
        }
    }

    private func submit(_ id: String) {
        let t = title.trimmingCharacters(in: .whitespaces)
        if !t.isEmpty { controller.reject(id, title: t) }
        editing = nil
    }

    private func crop(_ item: CatalogItem) -> some View {
        Group {
            if let data = item.images.first, let img = NSImage(data: data) {
                Image(nsImage: img).resizable().scaledToFit()
            } else {
                Rectangle().fill(.gray.opacity(0.3))
            }
        }
    }

    private func stateText(_ s: ItemState) -> String {
        switch s {
        case .unknown: return "no guess yet"
        case .guessed: return "guessed"
        case .confirmed(let by): return by == .barcode ? "confirmed (barcode)" : "confirmed"
        case .rejected: return "rejected"
        }
    }
}

struct StatsStrip: View {
    @ObservedObject var controller: DemoController

    var body: some View {
        HStack(spacing: 16) {
            Button(controller.running ? "Stop" : "Start") {
                Task { controller.running ? await controller.stop() : await controller.start() }
            }.keyboardShortcut(.space, modifiers: [])
            let s = controller.stats
            Text("frames in \(s.framesIn) · dropped \(s.framesDropped) · skipped \(s.framesSkipped) · processed \(s.framesProcessed) · items \(s.items)")
            Text("boxes \(controller.model.boxes.count) · hidden \(controller.model.hiddenCount)")
            Text(controller.latencyText)
            Text(controller.hitchText)
            Spacer()
        }
        .font(.caption.monospacedDigit())
        .padding(8)
        .overlay(alignment: .bottomLeading) {
            Text(controller.status).font(.caption2).foregroundColor(.secondary).padding(.leading, 8).offset(y: 14)
        }
        .padding(.bottom, 10)
    }
}
