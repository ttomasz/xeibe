// The loading screen: `#loading` in each page's HTML, shown from the first
// paint until the page's wasm module is running, with the download's
// progress. The page is unusable before that, and the modules are several
// megabytes.

/** Fetch a wasm module, telling `progress` the bytes so far. The Response
 * still streams, so the module compiles as it arrives. `size`: the module's
 * size when the page was built (vite.config.ts), 0 if unknown. GitHub Pages
 * serves wasm gzipped, and Content-Length is then the compressed size. */
export async function fetchWasm(
  url: string,
  size: number,
  progress: (bytes: number, total: number | null) => void,
): Promise<Response> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${response.status} ${response.statusText} fetching ${url}`);
  if (!response.body) return response;
  const length = response.headers.get('Content-Encoding') ? 0 : Number(response.headers.get('Content-Length'));
  const total = length || size || null;
  let bytes = 0;
  progress(bytes, total);
  const counter = new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      bytes += chunk.byteLength;
      progress(bytes, total);
      controller.enqueue(chunk);
    },
  });
  return new Response(response.body.pipeThrough(counter), { headers: { 'Content-Type': 'application/wasm' } });
}

const mb = (bytes: number) => (bytes / 1e6).toFixed(1);

function part<T extends HTMLElement>(selector: string): T | null {
  return document.querySelector<T>(`#loading ${selector}`);
}

export const loading = {
  /** Whether the screen is still up: a worker started again later loads
   * the module again, with no screen. */
  shown(): boolean {
    return !!document.getElementById('loading');
  },

  progress(bytes: number, total: number | null) {
    const bar = part<HTMLProgressElement>('progress');
    const what = part('.what');
    if (!bar || !what) return;
    if (total && bytes >= total) {
      // Downloaded; compiling the rest takes a moment.
      bar.removeAttribute('value');
      what.textContent = 'Starting…';
    } else if (total) {
      bar.value = bytes / total;
      what.textContent = `Downloading xeibe: ${mb(bytes)} of ${mb(total)} MB`;
    } else {
      what.textContent = `Downloading xeibe: ${mb(bytes)} MB`;
    }
  },

  done() {
    const screen = document.getElementById('loading');
    if (!screen) return;
    screen.classList.add('done');
    setTimeout(() => screen.remove(), 200);
  },

  failed(message: string) {
    const screen = document.getElementById('loading');
    if (!screen) return;
    screen.classList.add('failed');
    part('progress')?.remove();
    part('.what')!.textContent = "xeibe didn't load";
    const detail = part('.detail')!;
    detail.textContent = message;
    detail.classList.add('error');
    const reload = document.createElement('button');
    reload.className = 'button primary';
    reload.textContent = 'Reload';
    reload.addEventListener('click', () => location.reload());
    part('.box')!.append(reload);
  },
};
