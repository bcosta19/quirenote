import * as pdfjsLib from 'pdfjs-dist'
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import 'pdfjs-dist/web/pdf_viewer.css'

pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl
// pdf_viewer.mjs reads the library from globalThis.pdfjsLib.
;(globalThis as any).pdfjsLib = pdfjsLib

export { pdfjsLib }
export const loadViewer = () => import('pdfjs-dist/web/pdf_viewer.mjs')
