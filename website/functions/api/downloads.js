import { handleDownloadRequest } from '../../release-service.mjs';

export function onRequest(context) {
  return handleDownloadRequest(context.request, context, caches.default);
}
