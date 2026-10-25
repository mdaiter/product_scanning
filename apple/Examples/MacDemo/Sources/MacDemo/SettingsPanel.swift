import SwiftUI

/// Edits `controller.draft`. Nothing changes until Apply, which rebuilds the engine (plugins and config are fixed per engine).
struct SettingsPanel: View {
    @ObservedObject var controller: DemoController

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section("Models") {
                    Picker("Detector", selection: $controller.draft.detector) { ForEach(DetectorChoice.allCases) { Text($0.rawValue).tag($0) } }
                    if controller.draft.detector == .vision {
                        Toggle("Segmentation masks", isOn: $controller.draft.visionMasks)
                            .help("Vision instance masks. Off: boxes only (no cut-outs, slightly faster).")
                        IntRow(label: "Min instance size (px)", value: $controller.draft.visionMinPixels, range: 0...5000, step: 16,
                               help: "Smaller Vision instances are ignored (pixels in the mask buffer).")
                    } else {
                        IntRow(label: "Work size (px)", value: $controller.draft.blobWorkSide, range: 32...1024, step: 16,
                               help: "Long side the Blob detector analyses at. Larger finds smaller objects, costs more.")
                        FloatRow(label: "Colour threshold", value: $controller.draft.blobThreshold, range: 1...255, step: 1, format: "%.0f",
                                 help: "Difference from the border colour that counts as an object. Blob needs a plain table.")
                    }
                    Picker("Embedder", selection: $controller.draft.embedder) { ForEach(EmbedderChoice.allCases) { Text($0.rawValue).tag($0) } }
                    Picker("Barcode", selection: $controller.draft.scanner) { ForEach(ScannerChoice.allCases) { Text($0.rawValue).tag($0) } }
                }
                Section("Frames") {
                    FloatRow(label: "Analysis fps", value: $controller.draft.analysisFps, range: 1...30, step: 1, format: "%.0f",
                             help: "Frames per second admitted into the pipeline; the rest are skipped before copying.")
                    IntRow(label: "Analysis max side (px)", value: $controller.draft.analysisMaxSide, range: 240...1920, step: 80,
                           help: "Frames are downscaled to this long side before detection.")
                    FloatRow(label: "Novelty threshold", value: $controller.draft.noveltyThreshold, range: 0...5, step: 0.1, format: "%.1f",
                             help: "Skip a frame that differs less than this from the last one (0 = off). A still scene is mostly skipped.")
                    FloatRow(label: "Blur gate", value: $controller.draft.blurGateRatio, range: 0...1, step: 0.05, format: "%.2f",
                             help: "Skip frames sharper than less than this fraction of the running median (0 = off).")
                }
                Section("Detection") {
                    FloatRow(label: "Min score", value: $controller.draft.minScore, range: 0...1, step: 0.05, format: "%.2f", help: "Detections below this confidence are dropped.")
                    FloatRow(label: "NMS IoU", value: $controller.draft.nmsIou, range: 0.1...0.9, step: 0.05, format: "%.2f", help: "Overlapping boxes above this IoU are merged.")
                    FloatRow(label: "Min box area", value: $controller.draft.minBoxArea, range: 0...0.05, step: 0.001, format: "%.3f", help: "Boxes smaller than this fraction of the frame are ignored.")
                    FloatRow(label: "Background area", value: $controller.draft.backgroundArea, range: 0.1...1, step: 0.05, format: "%.2f",
                             help: "Boxes larger than this fraction of the frame become Background (hidden, counted in \"N hidden\"). Raise it if close-up items vanish.")
                    IntRow(label: "Max detections", value: $controller.draft.maxDetections, range: 1...1000, step: 10, help: "Per frame.")
                }
                Section("Tracking") {
                    IntRow(label: "Hits to become a product", value: $controller.draft.minTrackHits, range: 1...20, step: 1,
                           help: "A track is promoted (and guessed) after this many sightings. Lower shows products sooner.")
                    IntRow(label: "Max track age (frames)", value: $controller.draft.maxTrackAge, range: 1...60, step: 1, help: "Frames without a match before a track closes.")
                }
                Section("Clustering") {
                    FloatRow(label: "SKU similarity", value: $controller.draft.skuSimilarity, range: 0.5...1, step: 0.01, format: "%.2f",
                             help: "Cosine at which two tracks are the same product. Placeholder until the 1.8 bake-off calibrates it per embedder.")
                    FloatRow(label: "Fixture similarity", value: $controller.draft.fixtureSimilarity, range: 0.5...1, step: 0.01, format: "%.2f", help: "Auto-hide items this close to an excluded one.")
                    FloatRow(label: "Duplicate similarity", value: $controller.draft.duplicateSimilarity, range: 0.5...1, step: 0.01, format: "%.2f", help: "\"Already scanned?\" hint against earlier sessions.")
                }
                Section("Crops and identity") {
                    Toggle("Cut out onto white (uses masks)", isOn: $controller.draft.cutoutOnWhite)
                    IntRow(label: "Images per item", value: $controller.draft.maxImagesPerItem, range: 1...12, step: 1, help: "Best views kept per item.")
                    IntRow(label: "JPEG quality", value: $controller.draft.jpegQuality, range: 30...100, step: 5, help: "Crop JPEG quality.")
                    Toggle("Describe mode (no barcode prompts)", isOn: $controller.draft.describeMode)
                    IntRow(label: "Barcode votes", value: $controller.draft.barcodeVotes, range: 1...5, step: 1, help: "Agreeing frames needed to accept a barcode.")
                    IntRow(label: "Auto re-guesses", value: $controller.draft.guessRetries, range: 0...5, step: 1, help: "Re-guesses after a reject.")
                }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                Button("Defaults") { controller.draft = DemoSettings() }
                Button("Revert") { controller.draft = controller.settings }.disabled(!controller.draftChanged)
                Spacer()
                Button(controller.running ? "Apply (restarts scan)" : "Apply") { Task { await controller.apply() } }
                    .keyboardShortcut(.return, modifiers: .command)
                    .buttonStyle(.borderedProminent)
                    .disabled(!controller.draftChanged)
            }.padding(8)
        }
    }
}

struct FloatRow: View {
    let label: String
    @Binding var value: Float
    let range: ClosedRange<Float>
    let step: Float
    let format: String
    var help = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(label)
                Spacer()
                Text(String(format: format, value)).monospacedDigit().foregroundColor(.secondary)
            }
            Slider(value: $value, in: range, step: step).controlSize(.small)
        }.help(help)
    }
}

struct IntRow: View {
    let label: String
    @Binding var value: Int
    let range: ClosedRange<Int>
    let step: Int
    var help = ""

    var body: some View {
        Stepper(value: $value, in: range, step: step) {
            HStack {
                Text(label)
                Spacer()
                Text("\(value)").monospacedDigit().foregroundColor(.secondary)
            }
        }.help(help)
    }
}
