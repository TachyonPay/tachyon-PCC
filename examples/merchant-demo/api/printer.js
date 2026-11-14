export default async function handler(req, res) {
  res.setHeader("cache-control", "no-store");
  res.status(200).json({
    configured: true,
    online: false,
    cloud: true,
    camera: false,
    auto_start: false,
    error: "The A1 is on the Harborline LAN. Prints go out from Terminal.app (printer-agent), not this website.",
  });
}
