import AppKit
import ShelfScan
import ShelfScanCapture
import SwiftUI

struct ContentView: View {
    @ObservedObject var controller: DemoController
    @ObservedObject var model: ScanModel
    @State private var panel = Panel.items
    @AppStorage("showBoxes") private var showBoxes = true
    @AppStorage("showLabels") private var showLabels = true
    enum Panel { case items, settings }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ZStack(alignment: .topLeading) {
                    PreviewView(session: controller.session, frameSize: controller.frameSize, boxes: model.boxes,
                                showBoxes: showBoxes, showLabels: showLabels,
                                lensActive: controller.barcodeMode, lensQuads: controller.lensHits.map(\.corners))
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 6) {
                            Toggle("Barcode mode", isOn: Binding(get: { controller.barcodeMode },
                                                                 set: { on in Task { await controller.setBarcodeMode(on) } }))
                                .keyboardShortcut("b", modifiers: [.command, .shift])
                                .help("Read QR codes and barcodes from the raw camera picture, highlight and magnify them (⇧⌘B). Product scanning pauses.")
                            Toggle("Boxes", isOn: $showBoxes).keyboardShortcut("b", modifiers: .command).help("Show / hide boxes (⌘B)")
                            Toggle("Labels", isOn: $showLabels).keyboardShortcut("l", modifiers: .command).help("Show / hide labels (⌘L)")
                                .disabled(!showBoxes)
                            if model.hiddenCount > 0 {
                                Text("\(model.hiddenCount) hidden").font(.caption).padding(4).background(.black.opacity(0.6)).foregroundColor(.white)
                            }
                        }.toggleStyle(.button).controlSize(.small)
                        if controller.barcodeMode {
                            Text(controller.lensHits.isEmpty ? "Barcode mode: hold a QR code or barcode inside the square"
                                                             : "Reading \(controller.lensHits.count) code\(controller.lensHits.count == 1 ? "" : "s")…")
                                .font(.callout).padding(6).background(.green.opacity(0.85)).foregroundColor(.black)
                        } else if let id = controller.focused {
                            HStack {
                                Text("Scanning barcode for \(title(id)): hold the barcode up to the camera").font(.callout)
                                Button("Stop") { controller.stopBarcode() }
                            }.padding(6).background(.yellow.opacity(0.9)).foregroundColor(.black)
                        } else if let hint = model.barcodeHint {
                            Text("\(title(hint.itemId)) needs a barcode: press \"Scan barcode\" on it in the list, then show the barcode")
                                .font(.callout).padding(6).background(.orange.opacity(0.85)).foregroundColor(.black)
                        }
                    }.padding(8)
                    if controller.barcodeMode, let r = controller.lensRead {
                        LensCard(controller: controller, read: r)
                            .padding(12)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                            .transition(.scale(scale: 0.6, anchor: .bottomTrailing).combined(with: .opacity))
                            .id(r.id)
                    }
                }
                .animation(.spring(response: 0.3, dampingFraction: 0.8), value: controller.lensRead?.id)
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

    private func title(_ id: String) -> String {
        guard let i = controller.items.first(where: { $0.id == id }) else { return id }
        return i.guess.title.isEmpty ? id : "\"\(i.guess.title)\" (\(id))"
    }
}

struct ItemList: View {
    @ObservedObject var controller: DemoController
    @State private var editing: String?
    @State private var title = ""
    @State private var barcodeFor: String?
    @State private var digits = ""

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
                            if item.guess.confidence > 0 {
                                Text("confidence \(Int((item.guess.confidence * 100).rounded()))%").font(.caption2).foregroundColor(.secondary)
                            }
                            if let alt = item.guess.attributes.first(where: { $0.name == "Other guesses" }) {
                                Text("or: \(alt.value)").font(.caption2).foregroundColor(.secondary).lineLimit(2)
                            }
                            if item.needsBarcode { Text("needs barcode").font(.caption2).foregroundColor(.orange) }
                        }
                    }
                    HStack(spacing: 4) {
                        Button("✓") { controller.confirm(item.id) }.help("Confirm")
                        Button("✗") { controller.reject(item.id, title: nil) }.help("Reject: try another guess")
                        Button("Title…") { editing = item.id; title = "" }
                        Button("Hide") { controller.hide(item.id) }.help("Exclude this and everything like it")
                    }.buttonStyle(.bordered).controlSize(.small)
                    HStack(spacing: 4) {
                        if controller.focused == item.id {
                            Button("Stop scanning") { controller.stopBarcode() }
                        } else {
                            Button("Scan barcode") { controller.scanBarcode(item.id) }
                                .help("Focus this item, then hold its barcode up to the camera")
                                .disabled(!controller.running || item.gtin != nil)
                        }
                        Button("Barcode…") { barcodeFor = item.id; digits = "" }.help("Type the barcode digits")
                    }.buttonStyle(.bordered).controlSize(.small)
                    if barcodeFor == item.id {
                        HStack {
                            TextField("Barcode digits (EAN / UPC)", text: $digits).onSubmit { submitBarcode(item.id) }
                            Button("OK") { submitBarcode(item.id) }
                        }
                    }
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

    private func submitBarcode(_ id: String) {
        let d = digits.filter(\.isNumber)
        if !d.isEmpty { controller.typeBarcode(id, d) }
        barcodeFor = nil
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
            if controller.running && controller.settings.guesser.usesCloud {
                Text("☁ cloud on").foregroundColor(.orange).help("Masked product crops go to the Anthropic API")
            }
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

/// The magnified code and what it says (the WeChat-style zoom-in), with what can be done with it.
struct LensCard: View {
    @ObservedObject var controller: DemoController
    let read: DemoController.LensRead

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let img = read.image {
                Image(decorative: img, scale: 1).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                    .frame(maxWidth: 300, maxHeight: 220)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.green, lineWidth: 3))
                    .clipShape(RoundedRectangle(cornerRadius: 6))
            }
            Text(read.hit.payload).font(.system(.title3, design: .monospaced)).textSelection(.enabled).lineLimit(4)
            Text(kind).font(.caption).foregroundColor(.secondary)
            HStack {
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(read.hit.payload, forType: .string)
                }
                if read.hit.gtinValid == true {
                    Menu("Attach to item") {
                        ForEach(controller.items, id: \.id) { i in
                            Button("\(i.guess.title.isEmpty ? i.id : i.guess.title)\(i.needsBarcode ? "  (needs barcode)" : "")") {
                                controller.attach(read.hit, to: i.id)
                            }
                        }
                    }.disabled(controller.items.isEmpty).fixedSize()
                }
                Button("Dismiss") { controller.lensRead = nil }
            }.controlSize(.small)
            if controller.lensHistory.count > 1 {
                Divider()
                Text("Earlier").font(.caption2).foregroundColor(.secondary)
                ForEach(controller.lensHistory.dropFirst().prefix(4)) { h in
                    Text("\(h.hit.symbology)  \(h.hit.payload)").font(.caption2.monospaced()).lineLimit(1)
                }
            }
        }
        .padding(12)
        .frame(width: 330, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        .shadow(radius: 8)
    }

    private var kind: String {
        var parts = [read.hit.symbology]
        switch read.hit.gtinValid {
        case true?: parts.append("valid GTIN-\(read.hit.payload.count) check digit")
        case false?: parts.append("check digit WRONG: misread or not a GTIN")
        case nil: if let u = URL(string: read.hit.payload), u.scheme?.hasPrefix("http") == true { parts.append("link (not opened)") }
        }
        return parts.joined(separator: " · ")
    }
}
