// Browser side of Web Push (ADR-0017): feature detection, permission, subscribing this browser
// with the server's VAPID key, and remembering which server-side subscription is this device.

/** Why push cannot be used here, or "supported". */
export type PushSupport = "supported" | "insecure" | "ios-not-installed" | "unsupported";

/** Key in localStorage: the id of this browser's push subscription on the server. */
const DEVICE_KEY = "hm-push-subscription-id";

function isIos(): boolean {
  const ua = navigator.userAgent;
  // iPadOS reports itself as a Mac with touch.
  return /iPad|iPhone|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1);
}

function isStandalone(): boolean {
  const legacy = (navigator as Navigator & { standalone?: boolean }).standalone === true;
  return legacy || (window.matchMedia?.("(display-mode: standalone)").matches ?? false);
}

export function pushSupport(): PushSupport {
  if (!window.isSecureContext) return "insecure";
  const apis = "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
  if (apis) return "supported";
  // iOS/iPadOS 16.4+ offer Web Push only to apps added to the home screen.
  return isIos() && !isStandalone() ? "ios-not-installed" : "unsupported";
}

/** The notification permission ("default" where the API is missing). */
export function permission(): NotificationPermission {
  return "Notification" in window ? Notification.permission : "default";
}

/** The service worker registration; registers /sw.js if the page has none (dev server). */
async function registration(): Promise<ServiceWorkerRegistration> {
  const existing = await navigator.serviceWorker.getRegistration();
  if (!existing) await navigator.serviceWorker.register("/sw.js");
  return navigator.serviceWorker.ready;
}

/** base64url → bytes (the VAPID public key). */
export function decodeKey(base64url: string): Uint8Array<ArrayBuffer> {
  const base64 = base64url.replace(/-/g, "+").replace(/_/g, "/");
  const padded = base64 + "=".repeat((4 - (base64.length % 4)) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function sameKey(a: ArrayBuffer | null | undefined, b: Uint8Array): boolean {
  if (!a || a.byteLength !== b.length) return false;
  const view = new Uint8Array(a);
  return view.every((byte, i) => byte === b[i]);
}

/** The subscription JSON the API takes (`endpoint` and `keys`). */
export interface BrowserSubscription {
  endpoint: string;
  keys: { p256dh: string; auth: string };
}

/**
 * Asks for permission (must run from a click) and subscribes this browser with `vapidKey`.
 * An existing subscription for another key is replaced.
 */
export async function subscribe(vapidKey: string): Promise<BrowserSubscription> {
  const result = await Notification.requestPermission();
  if (result !== "granted") throw new PushDenied();
  const reg = await registration();
  const key = decodeKey(vapidKey);
  let sub = await reg.pushManager.getSubscription();
  if (sub && !sameKey(sub.options.applicationServerKey, key)) {
    await sub.unsubscribe();
    sub = null;
  }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  const json = sub.toJSON();
  if (!json.endpoint || !json.keys?.p256dh || !json.keys.auth) {
    throw new Error("The browser returned an incomplete push subscription.");
  }
  return { endpoint: json.endpoint, keys: { p256dh: json.keys.p256dh, auth: json.keys.auth } };
}

/** Removes this browser's push subscription, if any. */
export async function unsubscribe(): Promise<void> {
  const reg = await navigator.serviceWorker.getRegistration();
  const sub = await reg?.pushManager.getSubscription();
  await sub?.unsubscribe();
}

/** Whether this browser currently has a push subscription. */
export async function hasBrowserSubscription(): Promise<boolean> {
  if (pushSupport() !== "supported") return false;
  const reg = await navigator.serviceWorker.getRegistration();
  return Boolean(await reg?.pushManager.getSubscription());
}

/** The permission was not granted. */
export class PushDenied extends Error {
  constructor() {
    super("Notifications are blocked for this site.");
    this.name = "PushDenied";
  }
}

/** A short name for this device, e.g. "iPhone · Safari". */
export function deviceName(): string {
  const ua = navigator.userAgent;
  const device = /iPhone/.test(ua)
    ? "iPhone"
    : /iPad/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1)
      ? "iPad"
      : /Android/.test(ua)
        ? "Android"
        : /Macintosh|Mac OS X/.test(ua)
          ? "Mac"
          : /Windows/.test(ua)
            ? "Windows"
            : /Linux/.test(ua)
              ? "Linux"
              : "Device";
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /Firefox\//.test(ua)
      ? "Firefox"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Safari\//.test(ua)
          ? "Safari"
          : null;
  return browser ? `${device} · ${browser}` : device;
}

/** The server-side subscription id of this browser (per-browser convenience, may be absent). */
export function thisDevice(): string | null {
  try {
    return localStorage.getItem(DEVICE_KEY);
  } catch {
    return null;
  }
}

export function rememberDevice(id: string | null): void {
  try {
    if (id) localStorage.setItem(DEVICE_KEY, id);
    else localStorage.removeItem(DEVICE_KEY);
  } catch {
    // Private mode or blocked storage: the device is just not marked as "this device".
  }
}
