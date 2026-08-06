// Offscreen WebKit HTML -> PNG snapshotter.
//
// Usage: webkit_snap <input.html> <output.png> [scale]
//
// Loads the HTML file in a WKWebView (same engine Safari and Dash use),
// waits for layout, sizes the view to the bounding box of the element
// with id="snap" (falling back to the document body), and writes a PNG.
// Built and driven by render_mathml.py.

import AppKit
import WebKit

let args = CommandLine.arguments
guard args.count >= 3 else {
    FileHandle.standardError.write("usage: webkit_snap in.html out.png [scale]\n".data(using: .utf8)!)
    exit(2)
}
let inURL = URL(fileURLWithPath: args[1])
let outURL = URL(fileURLWithPath: args[2])
let scale: CGFloat = args.count > 3 ? CGFloat(Double(args[3]) ?? 2.0) : 2.0

final class Delegate: NSObject, WKNavigationDelegate {
    let webView: WKWebView
    init(_ w: WKWebView) { webView = w }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // Give fonts/layout a beat to settle before measuring.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { self.measure() }
    }

    func measure() {
        let js = """
        (function () {
          var el = document.getElementById('snap') || document.body;
          var r = el.getBoundingClientRect();
          return [Math.ceil(r.x + r.width), Math.ceil(r.y + r.height)];
        })()
        """
        webView.evaluateJavaScript(js) { result, _ in
            var w: CGFloat = 800, h: CGFloat = 600
            if let dims = result as? [Any], dims.count == 2,
               let dw = dims[0] as? NSNumber, let dh = dims[1] as? NSNumber {
                w = max(CGFloat(truncating: dw), 8)
                h = max(CGFloat(truncating: dh), 8)
            }
            self.webView.frame = NSRect(x: 0, y: 0, width: w + 8, height: h + 8)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { self.snap() }
        }
    }

    func snap() {
        let config = WKSnapshotConfiguration()
        config.snapshotWidth = NSNumber(value: Double(webView.frame.width) * Double(scale) / 2.0)
        webView.takeSnapshot(with: config) { image, error in
            guard let image = image,
                  let tiff = image.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff),
                  let png = rep.representation(using: .png, properties: [:]) else {
                FileHandle.standardError.write("snapshot failed: \(error.map(String.init(describing:)) ?? "?")\n".data(using: .utf8)!)
                exit(1)
            }
            try? png.write(to: outURL)
            exit(0)
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        FileHandle.standardError.write("load failed: \(error)\n".data(using: .utf8)!)
        exit(1)
    }
}

let webView = WKWebView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
let delegate = Delegate(webView)
webView.navigationDelegate = delegate
webView.loadFileURL(inURL, allowingReadAccessTo: URL(fileURLWithPath: "/"))

// Hard timeout so a wedged load can't hang the pipeline.
DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
    FileHandle.standardError.write("timeout\n".data(using: .utf8)!)
    exit(1)
}
RunLoop.main.run()
