import * as pdfjsLib from 'pdfjs-dist'
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import 'pdfjs-dist/web/pdf_viewer.css'

pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl
// O pdf_viewer.mjs lê a biblioteca de globalThis.pdfjsLib.
;(globalThis as any).pdfjsLib = pdfjsLib

export { pdfjsLib }
export const loadViewer = () => import('pdfjs-dist/web/pdf_viewer.mjs')
