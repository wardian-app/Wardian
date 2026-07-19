import PdfJsWorker from "pdfjs-dist/build/pdf.worker.min.mjs?worker";

type PdfJsWorkerOptions = {
  GlobalWorkerOptions: { workerPort: Worker | null };
};

let worker: Worker | null = null;

/** Connects PDF.js to Wardian's Vite-bundled local worker exactly once. */
export function configurePdfWorker(pdfjs: PdfJsWorkerOptions) {
  worker ??= new PdfJsWorker({ name: "wardian-pdfjs" });
  if (pdfjs.GlobalWorkerOptions.workerPort !== worker) {
    pdfjs.GlobalWorkerOptions.workerPort = worker;
  }
}
