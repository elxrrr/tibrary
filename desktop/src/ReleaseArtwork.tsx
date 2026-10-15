import { useEffect, useRef, useState } from "react";
import { Disc3, ListMusic, UserRound } from "lucide-react";
import { call } from "./api";

type Thumbnail = { src: string; cover_id: string; size: number };
type Subscriber = {
  visible: () => boolean;
  receive: (thumbnail: Thumbnail | null) => void;
  skipped: () => void;
};
type Request = {
  key: string;
  releaseId?: string;
  imageId?: string;
  market?: string;
  started: boolean;
  subscribers: Set<Subscriber>;
};

const CACHE_LIMIT = 160;
const REQUEST_LIMIT = 4;
// Keep successful images across row unmounts and route changes. Missing metadata
// is deliberately not cached: a later table refresh may provide a cover ID.
const thumbnails = new Map<string, Thumbnail>();
const requests = new Map<string, Request>();
const visibilityCallbacks = new Map<Element, (visible: boolean) => void>();
let observer: IntersectionObserver | undefined;
let activeRequests = 0;
let scheduled = false;

function cachedThumbnail(key: string) {
  const thumbnail = thumbnails.get(key);
  if (thumbnail) {
    thumbnails.delete(key);
    thumbnails.set(key, thumbnail);
  }
  return thumbnail;
}

function deliverVisibility(entries: IntersectionObserverEntry[]) {
  for (const entry of entries)
    visibilityCallbacks.get(entry.target)?.(
      entry.isIntersecting && entry.intersectionRect.width > 0 && entry.intersectionRect.height > 0,
    );
}

function observeVisibility(element: Element, callback: (visible: boolean) => void) {
  // The viewport root also accounts for the table's clipping ancestors. A
  // table-only root would incorrectly fetch when the whole table is offscreen.
  // The positive threshold reports entry after an exactly edge-adjacent,
  // zero-area intersection (which must not start an artwork request).
  observer ??= new IntersectionObserver(deliverVisibility, { root: null, rootMargin: "0px", threshold: [0, 0.001] });
  visibilityCallbacks.set(element, callback);
  observer.observe(element);
  return () => {
    observer?.unobserve(element);
    visibilityCallbacks.delete(element);
    if (!visibilityCallbacks.size) {
      observer?.disconnect();
      observer = undefined;
    }
  };
}

function withinVisibleBounds(element: HTMLElement) {
  if (!element.isConnected) return false;
  const bounds = element.getBoundingClientRect();
  let left = Math.max(0, bounds.left);
  let top = Math.max(0, bounds.top);
  let right = Math.min(window.innerWidth, bounds.right);
  let bottom = Math.min(window.innerHeight, bounds.bottom);
  // Recheck queued work immediately before dispatch: scrolling can happen
  // between an intersection notification and an earlier request completing.
  for (let parent = element.parentElement; parent && left < right && top < bottom; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    const clipsX = /auto|scroll|hidden|clip/.test(style.overflowX);
    const clipsY = /auto|scroll|hidden|clip/.test(style.overflowY);
    if (!clipsX && !clipsY) continue;
    const clip = parent.getBoundingClientRect();
    if (clipsX) { left = Math.max(left, clip.left); right = Math.min(right, clip.right); }
    if (clipsY) { top = Math.max(top, clip.top); bottom = Math.min(bottom, clip.bottom); }
  }
  return left < right && top < bottom;
}

function scheduleRequests() {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(() => {
    scheduled = false;
    deliverVisibility(observer?.takeRecords() || []);
    for (const request of requests.values()) {
      if (activeRequests >= REQUEST_LIMIT) break;
      if (request.started) continue;
      for (const subscriber of request.subscribers) {
        if (!subscriber.visible()) {
          request.subscribers.delete(subscriber);
          subscriber.skipped();
        }
      }
      if (!request.subscribers.size) {
        requests.delete(request.key);
        continue;
      }
      request.started = true;
      activeRequests++;
      void call<Thumbnail | null>(request.imageId ? "artwork.image" : "artwork.thumbnail", {
        ...(request.imageId ? {image_id: request.imageId} : {release_id: request.releaseId}),
        ...(request.market ? { market: request.market } : {}),
      }).catch(() => null).then(thumbnail => {
        if (thumbnail?.src?.startsWith("data:image/jpeg;base64,")) {
          thumbnails.delete(request.key);
          thumbnails.set(request.key, thumbnail);
          while (thumbnails.size > CACHE_LIMIT) thumbnails.delete(thumbnails.keys().next().value!);
        } else {
          thumbnail = null;
        }
        requests.delete(request.key);
        activeRequests--;
        for (const subscriber of request.subscribers) subscriber.receive(thumbnail);
        scheduleRequests();
      });
    }
  });
}

function requestThumbnail(key: string, releaseId: string | undefined, imageId: string | undefined, market: string | undefined, subscriber: Subscriber) {
  let request = requests.get(key);
  if (!request) {
    request = { key, releaseId, imageId, market, started: false, subscribers: new Set() };
    requests.set(key, request);
  }
  request.subscribers.add(subscriber);
  scheduleRequests();
  return () => {
    request.subscribers.delete(subscriber);
    if (!request.started && !request.subscribers.size) requests.delete(key);
  };
}

function Artwork({ releaseId, imageId, kind = "release", market, metadata }: {
  releaseId?: string;
  imageId?: string | null;
  kind?: "release" | "artist" | "playlist";
  market?: string;
  /** A replacement row means cached metadata may now include artwork. */
  metadata: object;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const key = JSON.stringify([market || "", kind === "release" ? "release" : "image", releaseId || imageId || ""]);
  const [image, setImage] = useState<{ key: string; src: string } | null>(null);
  useEffect(() => {
    const element = ref.current;
    if (!element || (!releaseId && !imageId)) return;
    let visible = false;
    let attempted = false;
    let cancel: (() => void) | undefined;
    // A table refresh may replace metadata without changing this release.
    // Keep its loaded image until the observer confirms it is offscreen.
    setImage(previous => previous?.key === key ? previous : null);
    const stopObserving = observeVisibility(element, nextVisible => {
      visible = nextVisible;
      if (!visible) {
        cancel?.();
        cancel = undefined;
        // Offscreen mounted rows must not retain extra image data beyond the LRU.
        setImage(null);
        return;
      }
      const cached = cachedThumbnail(key);
      if (cached) {
        setImage({ key, src: cached.src });
        return;
      }
      if (attempted || cancel) return;
      cancel = requestThumbnail(key, releaseId, imageId || undefined, market, {
        visible: () => visible && withinVisibleBounds(element),
        skipped: () => { cancel = undefined; },
        receive: thumbnail => {
          cancel = undefined;
          // A successful row can fall out of the LRU while it remains mounted
          // offscreen; let it reload from the disk cache when it returns.
          attempted = !thumbnail;
          if (visible && thumbnail) setImage({ key, src: thumbnail.src });
        },
      });
    });
    return () => { visible = false; stopObserving(); cancel?.(); };
  }, [key, releaseId, imageId, market, metadata]);

  const src = image?.key === key ? image.src : undefined;
  return <span ref={ref} className={`release-artwork${kind === "release" ? "" : ` catalogue-artwork artwork-${kind}`}`} data-release-id={releaseId} data-image-id={imageId || undefined} aria-hidden="true">
    {src ? <img src={src} width={48} height={48} alt="" decoding="async" onError={() => {
      if (thumbnails.get(key)?.src === src) thumbnails.delete(key);
      setImage(null);
    }} /> : kind === "artist" ? <UserRound size={20} strokeWidth={1.25} /> : kind === "playlist" ? <ListMusic size={20} strokeWidth={1.25} /> : <Disc3 size={20} strokeWidth={1.25} />}
  </span>;
}

export function ReleaseArtwork(props: {releaseId: string; market?: string; metadata: object}) {
  return <Artwork {...props} />;
}

/** Reuses the visible-only thumbnail lane for catalogue portraits and covers. */
export function CatalogueArtwork(props: {imageId?: string | null; kind: "artist" | "playlist"; metadata: object}) {
  return <Artwork {...props} />;
}
