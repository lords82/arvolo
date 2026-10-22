// What another app handed over through the system share sheet.
//
// An iOS share extension is a separate process with its own sandbox: it cannot
// touch the app's files and cannot call into the Rust library. What it can do is
// write into the App Group container both halves can read, and ask the system to
// open the app. So: every attachment is copied under `shared/<uuid>/` in the
// group, then `arvolo://shared` is opened, and the app — on that URL, on coming
// to the front, or at the next launch — moves what it finds there into its own
// storage and opens Scambia with the files already in hand (the design's 11e).
//
// Nothing is decrypted, sent or decided here. The extension is a courier.

import UIKit
import UniformTypeIdentifiers

private let appGroup = "group.it.termox.arvolo"

class ShareViewController: UIViewController {
    private let label = UILabel()
    private var pending = 0
    private var dir: URL?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        label.text = "Arvolo"
        label.font = .preferredFont(forTextStyle: .headline)
        label.textAlignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        collect()
    }

    private func collect() {
        guard let base = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup) else {
            return finish()
        }
        let dir = base.appendingPathComponent("shared/\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        self.dir = dir

        let items = (extensionContext?.inputItems as? [NSExtensionItem]) ?? []
        let attachments = items.flatMap { $0.attachments ?? [] }
        if attachments.isEmpty { return finish() }
        pending = attachments.count
        for (i, item) in attachments.enumerated() {
            take(item, index: i, into: dir)
        }
    }

    /// One attachment, by the most file-like thing it can be. A file URL or any
    /// data is copied as it is; a bare URL or a piece of text becomes a `.txt`,
    /// because files are what the engine sends.
    private func take(_ item: NSItemProvider, index: Int, into dir: URL) {
        let fileish = [UTType.fileURL, .image, .movie, .audio, .pdf, .data]
        if let t = fileish.first(where: { item.hasItemConformingToTypeIdentifier($0.identifier) }) {
            item.loadFileRepresentation(forTypeIdentifier: t.identifier) { url, _ in
                if let url = url {
                    let name = url.lastPathComponent.isEmpty ? "file-\(index + 1)" : url.lastPathComponent
                    let dest = Self.unclaimed(dir, name)
                    try? FileManager.default.copyItem(at: url, to: dest)
                }
                self.done()
            }
            return
        }
        for t in [UTType.url, .plainText, .text] where item.hasItemConformingToTypeIdentifier(t.identifier) {
            item.loadItem(forTypeIdentifier: t.identifier) { value, _ in
                let text: String? = (value as? URL)?.absoluteString ?? (value as? String)
                    ?? (value as? Data).flatMap { String(data: $0, encoding: .utf8) }
                if let text = text, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    let dest = Self.unclaimed(dir, "testo-\(index + 1).txt")
                    try? text.write(to: dest, atomically: true, encoding: .utf8)
                }
                self.done()
            }
            return
        }
        done()
    }

    private func done() {
        DispatchQueue.main.async {
            self.pending -= 1
            if self.pending <= 0 { self.finish() }
        }
    }

    private func finish() {
        if let dir = dir, (try? FileManager.default.contentsOfDirectory(atPath: dir.path))?.isEmpty ?? true {
            try? FileManager.default.removeItem(at: dir)
        }
        openApp()
        extensionContext?.completeRequest(returningItems: nil)
    }

    /// Ask the system to bring Arvolo up. Extensions have no `UIApplication`;
    /// the responder chain still reaches the one that hosts the sheet, and it
    /// answers `openURL:`. Where it does not, the files wait in the group and
    /// the app finds them the next time it comes to the front.
    private func openApp() {
        guard let url = URL(string: "arvolo://shared") else { return }
        var r: UIResponder? = self
        while let next = r {
            if next.responds(to: #selector(UIApplication.open(_:options:completionHandler:))) {
                next.perform(#selector(UIApplication.open(_:options:completionHandler:)), with: url, with: nil)
                return
            }
            r = next.next
        }
    }

    private static func unclaimed(_ dir: URL, _ name: String) -> URL {
        var url = dir.appendingPathComponent(name)
        var n = 2
        let ext = (name as NSString).pathExtension
        let stem = (name as NSString).deletingPathExtension
        while FileManager.default.fileExists(atPath: url.path) {
            url = dir.appendingPathComponent(ext.isEmpty ? "\(stem)-\(n)" : "\(stem)-\(n).\(ext)")
            n += 1
        }
        return url
    }
}
