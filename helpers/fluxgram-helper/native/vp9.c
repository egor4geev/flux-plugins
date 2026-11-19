/* fluxgram-helper's door to libvpx's VP9 decoder (src/webm.rs): a few plain functions, so the Rust
 * side declares none of libvpx's structures (their layout, `long` on Windows, the ABI versions). */

#include <limits.h>
#include <stdlib.h>

#include "vpx/vp8dx.h"
#include "vpx/vpx_decoder.h"

typedef struct {
  vpx_codec_ctx_t codec;
  vpx_codec_iter_t iter;
} fluxgram_vp9;

/* A decoded picture: 8-bit Y, U, V planes (U and V subsampled by the shifts), valid until the
 * decoder's next call. */
typedef struct {
  const unsigned char *planes[3];
  int strides[3];
  unsigned int width;
  unsigned int height;
  unsigned int x_shift;
  unsigned int y_shift;
  /* vpx_color_space_t: 0 unknown, 1 BT.601, 2 BT.709, 3 SMPTE 170, 4 SMPTE 240, 5 BT.2020, 7 sRGB. */
  unsigned int color_space;
  /* 1: full range (0–255), 0: studio (16–235). */
  unsigned int full_range;
} fluxgram_vp9_picture;

/* A decoder on the calling thread (no threads of its own); NULL if there's no memory. */
void *fluxgram_vp9_new(void) {
  vpx_codec_dec_cfg_t cfg = {1, 0, 0};
  fluxgram_vp9 *decoder = calloc(1, sizeof(*decoder));
  if (decoder == NULL) return NULL;
  if (vpx_codec_dec_init(&decoder->codec, vpx_codec_vp9_dx(), &cfg, 0) != VPX_CODEC_OK) {
    free(decoder);
    return NULL;
  }
  return decoder;
}

/* Decodes a frame (a block's data): 0, or -1 with the reason in fluxgram_vp9_error. */
int fluxgram_vp9_decode(void *handle, const unsigned char *data, size_t size) {
  fluxgram_vp9 *decoder = handle;
  decoder->iter = NULL;
  if (size == 0 || size > UINT_MAX) return -1;
  return vpx_codec_decode(&decoder->codec, data, (unsigned int)size, NULL, 0) == VPX_CODEC_OK
             ? 0
             : -1;
}

/* The picture the last decode showed: 1, 0 if it showed none, -1 if it isn't 8-bit. */
int fluxgram_vp9_picture_get(void *handle, fluxgram_vp9_picture *picture) {
  fluxgram_vp9 *decoder = handle;
  vpx_image_t *image = NULL;
  vpx_image_t *next;
  /* A superframe shows one picture; the last one given is it. */
  while ((next = vpx_codec_get_frame(&decoder->codec, &decoder->iter)) != NULL) image = next;
  if (image == NULL) return 0;
  if (image->fmt & VPX_IMG_FMT_HIGHBITDEPTH) return -1;
  for (int plane = 0; plane < 3; plane++) {
    picture->planes[plane] = image->planes[plane];
    picture->strides[plane] = image->stride[plane];
  }
  picture->width = image->d_w;
  picture->height = image->d_h;
  picture->x_shift = image->x_chroma_shift;
  picture->y_shift = image->y_chroma_shift;
  picture->color_space = (unsigned int)image->cs;
  picture->full_range = image->range == VPX_CR_FULL_RANGE;
  return 1;
}

/* Why the last call failed. */
const char *fluxgram_vp9_error(void *handle) {
  fluxgram_vp9 *decoder = handle;
  const char *detail = vpx_codec_error_detail(&decoder->codec);
  return detail != NULL ? detail : vpx_codec_error(&decoder->codec);
}

void fluxgram_vp9_free(void *handle) {
  fluxgram_vp9 *decoder = handle;
  if (decoder == NULL) return;
  vpx_codec_destroy(&decoder->codec);
  free(decoder);
}
