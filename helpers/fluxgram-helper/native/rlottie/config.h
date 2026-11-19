/* rlottie's configuration for fluxgram-helper: its CMake's cmake/config.h.in with every option off.
 *
 * - No LOTTIE_THREAD: no pool of rasterizing threads (rlottie's own, one per core); a frame is
 *   rendered on the calling thread. rlottie isn't reentrant so: the program renders one Lottie at a
 *   time (src/tgs.rs).
 * - No LOTTIE_CACHE: no process-wide cache of parsed animations — each render parses its sticker
 *   once and frees it.
 * - No LOTTIE_MODULE: a Lottie's pictures (rare in stickers) are decoded by stb_image compiled in, not
 *   by a plugin library loaded at run time.
 */
