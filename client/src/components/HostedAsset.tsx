import { useEffect, useRef, useState, type ImgHTMLAttributes, type VideoHTMLAttributes, type AnchorHTMLAttributes } from 'react';
import { hostedRoute } from '@/lib/host-tunnel';
import { transportFetch } from '@/api/hosted-fetch';

function useHostedAsset(source: string | undefined) {
  const hosted = !!source && !!hostedRoute(source);
  const [loaded, setLoaded] = useState<{ source: string; url: string } | null>(null);
  const [failed, setFailed] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!hosted || !source) return;
    const abort = new AbortController(); let objectUrl: string | undefined;
    void transportFetch(source, { signal: abort.signal, redirect: 'error' }).then(async response => {
      if (!response.ok) throw new Error('Hosted asset unavailable');
      const blob = await response.blob();
      if (abort.signal.aborted) return;
      objectUrl = URL.createObjectURL(blob); setLoaded({ source, url: objectUrl });
    }).catch(() => { if (!abort.signal.aborted) setFailed(source); });
    return () => { abort.abort(); if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [source, hosted, attempt]);
  return { url: hosted ? loaded?.source === source ? loaded.url : undefined : source,
    error: hosted && failed === source, retry: () => { setFailed(undefined); setAttempt(value => value + 1); } };
}

export function HostedImage({ src, ...props }: ImgHTMLAttributes<HTMLImageElement>) {
  const asset = useHostedAsset(src);
  return asset.error ? <button type="button" onClick={asset.retry} title="The image could not be loaded. Retry the encrypted download.">Retry image</button> : <img {...props} src={asset.url} />;
}
export function HostedVideo({ src, ...props }: VideoHTMLAttributes<HTMLVideoElement>) {
  const asset = useHostedAsset(src);
  return asset.error ? <button type="button" onClick={asset.retry}>Retry video</button> : <video {...props} src={asset.url} />;
}
export function HostedDownload({ href, children, onClick, ...props }: AnchorHTMLAttributes<HTMLAnchorElement>) {
  const [state, setState] = useState<'idle' | 'loading' | 'failed'>('idle');
  const active = useRef<AbortController | null>(null);
  const objectUrls = useRef(new Set<string>());
  useEffect(() => {
    const urls = objectUrls.current;
    return () => { active.current?.abort(); for (const url of urls) URL.revokeObjectURL(url); urls.clear(); };
  }, [href]);
  if (!href || !hostedRoute(href)) return <a {...props} href={href} onClick={onClick}>{children}</a>;
  return <a {...props} href="#" aria-busy={state === 'loading'} onClick={event => {
    onClick?.(event); if (event.defaultPrevented) return; event.preventDefault();
    if (active.current) return;
    const abort = new AbortController(); active.current = abort; setState('loading');
    void transportFetch(href, { signal: abort.signal, redirect: 'error' }).then(async response => {
      if (!response.ok) throw new Error('Download unavailable');
      const blob = await response.blob(); if (abort.signal.aborted) return;
      const url = URL.createObjectURL(blob); objectUrls.current.add(url);
      const link = document.createElement('a'); link.href = url;
      link.download = typeof props.download === 'string' ? props.download : new URL(href).pathname.split('/').pop() || 'download';
      link.click(); setState('idle');
      setTimeout(() => { URL.revokeObjectURL(url); objectUrls.current.delete(url); }, 60_000);
    }).catch(() => { if (!abort.signal.aborted) setState('failed'); }).finally(() => { if (active.current === abort) active.current = null; });
  }}>{children}{state === 'loading' ? ' — Downloading…' : state === 'failed' ? ' — Download failed. Click to retry.' : ''}</a>;
}
