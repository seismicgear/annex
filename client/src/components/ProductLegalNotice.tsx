import { useState, type ReactNode } from 'react';
import { Modal } from './Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';
import notice from '../../PRODUCT_NOTICE.md?raw';
import './ProductLegalNotice.css';

// The bundled notice is also the source distribution's customer document.
// Render only its headings, paragraphs and links; never interpret HTML.
function inline(text: string): ReactNode[] {
  return text.split(/(\[[^\]]+\]\([^)]+\))/g).map((part, index) => {
    const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(part);
    if (!link) return part;
    const url = link[2] === 'LICENSE.md'
      ? 'https://github.com/seismicgear/annex/blob/main/LICENSE.md'
      : link[2];
    if (!url.startsWith('https://')) return link[1];
    return <a key={index} href={url} target="_blank" rel="noopener noreferrer">{link[1]}</a>;
  });
}

export function ProductLegalNotice() {
  const [open, setOpen] = useState(false);
  const titleId = useDialogTitleId();
  return <>
    <button type="button" className="secondary-btn product-legal-trigger" onClick={() => setOpen(true)}>Privacy &amp; license</button>
    {open && <Modal onClose={() => setOpen(false)} titleId={titleId} className="product-legal-dialog">
      <div data-legal-version="2026-11-07-portfolio-1">
        {notice.trim().split(/\n\n+/).map((block, index) => {
          if (block.startsWith('# ')) return <h2 id={titleId} key={index}>{block.slice(2)}</h2>;
          if (block.startsWith('## ')) return <h3 key={index}>{block.slice(3)}</h3>;
          return <p key={index}>{inline(block)}</p>;
        })}
      </div>
      <div className="dialog-actions"><button type="button" onClick={() => setOpen(false)}>Close</button></div>
    </Modal>}
  </>;
}
