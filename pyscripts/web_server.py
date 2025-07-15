#!/usr/bin/env python3

from http.server import SimpleHTTPRequestHandler
from pathlib import Path
import os
import sys
import argparse
import subprocess
import ssl
import socketserver


class CORSRequestHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()


def shell_open(url):
    if sys.platform == "win32":
        os.startfile(url)
    else:
        opener = "open" if sys.platform == "darwin" else "xdg-open"
        subprocess.call([opener, url])


def serve(root, port, run_browser):
    os.chdir(root)

    if run_browser:
        # Open the served page in the user's default browser.
        print("Opening the served URL in the default browser (use `--no-browser` or `-n` to disable this).")
        shell_open(f"https://localhost:{port}")
    
    Handler = CORSRequestHandler

    script_dir = Path(__file__).resolve().parent
    cert_path = script_dir / "server.crt"
    key_path = script_dir / "server.key"

    print(f"cert: {cert_path}, key: {key_path}")
    with socketserver.TCPServer(("", port), Handler) as httpd:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certfile=str(cert_path), keyfile=str(key_path))
        httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
        print(f"serving at https://localhost:{port}")
        httpd.serve_forever()

    # test(CORSRequestHandler, HTTPServer, port=port)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("-p", "--port", help="port to listen on", default=8069, type=int)
    parser.add_argument(
        "-r", "--root", help="path to serve as root (relative to `platform/web/`)", default="../build/3dsim/", type=Path
    )
    browser_parser = parser.add_mutually_exclusive_group(required=False)
    browser_parser.add_argument(
        "-n", "--no-browser", help="don't open default web browser automatically", dest="browser", action="store_false"
    )
    parser.set_defaults(browser=True)
    args = parser.parse_args()

    # Change to the directory where the script is located,
    # so that the script can be run from any location.
    os.chdir(Path(__file__).resolve().parent)

    serve(args.root, args.port, args.browser)