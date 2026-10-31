/* SPDX-License-Identifier: MIT OR Apache-2.0 */
/*
 * Smoke test of the C ABI: opens an image, prints its areas and tracks, and decodes every
 * track that has audio, on the calling thread and again on a pool of two threads, which
 * must give the same frames. Exits 0 on success, 1 on an unexpected error.
 *
 *   cc -std=c99 -Wall -Wextra -Werror -pedantic -Iinclude examples/smoke.c \
 *      ../../target/release/libscarletbook_ffi.a -lpthread -ldl -lm -o smoke
 */
#include <stdio.h>
#include <stdlib.h>
#include "scarletbook.h"

static int fail(const char *what, SbStatus s) {
    fprintf(stderr, "%s: %s (%s)\n", what, sb_status_name(s), sb_last_error_message());
    return 1;
}

struct Decoded {
    uint64_t frames, missing, invalid, sum;
};

/* Decodes track t of the area from slot (on pool, if not NULL) into *out: the counts and an
 * FNV-1a hash of every frame's bytes. Returns SB_STATUS_OK, or the failing call's status. */
static SbStatus decode(struct SbDisc *disc, uint32_t slot, uint32_t t, struct SbPool *pool,
                            struct Decoded *out) {
    struct SbTrackAudio *audio = NULL;
    struct SbFrameInfo fi;
    uint8_t *buf;
    size_t cap, i;
    SbStatus s = sb_track_audio_open(disc, slot, t, &audio);
    if (s != SB_STATUS_OK) return s;
    if (pool != NULL && (s = sb_track_audio_set_pool(audio, pool)) != SB_STATUS_OK) {
        sb_track_audio_free(audio);
        return s;
    }
    out->frames = 0;
    out->sum = 14695981039346656037ULL;
    cap = (size_t)sb_track_audio_channels(audio) * 4704;
    buf = malloc(cap);
    if (buf == NULL) exit(1);
    while ((s = sb_track_audio_read(audio, disc, buf, cap, &fi)) == SB_STATUS_OK) {
        out->frames++;
        for (i = 0; i < fi.size; i++) out->sum = (out->sum ^ buf[i]) * 1099511628211ULL;
    }
    free(buf);
    if (s == SB_STATUS_END) s = sb_track_audio_counts(audio, &out->missing, &out->invalid);
    sb_track_audio_free(audio);
    return s;
}

int main(int argc, char **argv) {
    struct SbDisc *disc = NULL;
    struct SbPool *pool = NULL;
    struct SbDiscInfo info;
    SbStatus s;
    uint32_t a;
    char title[256];
    size_t len = 0;

    if (argc != 2) {
        fprintf(stderr, "usage: smoke IMAGE\n");
        return 2;
    }
    /* The library must implement the ABI this program was compiled against. */
    if (sb_abi_version() != SB_ABI_VERSION) {
        fprintf(stderr, "library ABI %u, header ABI %u\n", sb_abi_version(), (unsigned)SB_ABI_VERSION);
        return 1;
    }
    printf("scarletbook %s\n", sb_version());
    {
        /* ABI 2: a DST frame longer than SB_MAX_FRAME_BYTES is refused before it is read. */
        uint8_t frame[1] = {0}, out[4704];
        if ((s = sb_dst_decode(frame, (size_t)SB_MAX_FRAME_BYTES + 1, 1, out, sizeof out, NULL)) !=
            SB_STATUS_INVALID_ARGUMENT)
            return fail("sb_dst_decode of an oversized frame", s);
    }
    if ((s = sb_pool_new(2, &pool)) != SB_STATUS_OK) return fail("sb_pool_new", s);
    s = sb_disc_open_file(argv[1], &disc);
    if (s != SB_STATUS_OK) {
        /* A rejected image is a valid outcome, not a failure of the library. */
        printf("rejected: %s\n", sb_status_name(s));
        sb_pool_free(pool);
        return s < SB_STATUS_NO_AUDIO_AREAS ? 1 : 0;
    }
    if ((s = sb_disc_info(disc, &info)) != SB_STATUS_OK) return fail("sb_disc_info", s);
    printf("sector size %u, %llu sectors, %u areas, %u warnings, hybrid %u\n", info.sector_size,
           (unsigned long long)info.sectors, info.area_count, info.warning_count, info.hybrid);
    s = sb_disc_string(disc, SB_DISC_STRING_ALBUM_TITLE, title, sizeof title, &len);
    if (s == SB_STATUS_OK) printf("album title: %s\n", title);

    for (a = 0; a < info.area_count; a++) {
        struct SbAreaInfo area;
        uint32_t t;
        if ((s = sb_disc_area(disc, a, &area)) != SB_STATUS_OK) return fail("sb_disc_area", s);
        printf("area slot %u: %u channels, %u tracks\n", area.slot, area.channel_count, area.track_count);
        for (t = 1; t <= area.track_count; t++) {
            struct Decoded here, pooled;
            if ((s = sb_track_check(disc, area.slot, t)) != SB_STATUS_OK) {
                printf("  track %u: no audio (%s)\n", t, sb_status_name(s));
                continue;
            }
            if ((s = decode(disc, area.slot, t, NULL, &here)) != SB_STATUS_OK) return fail("decoding", s);
            if ((s = decode(disc, area.slot, t, pool, &pooled)) != SB_STATUS_OK) return fail("decoding on a pool", s);
            if (here.frames != pooled.frames || here.missing != pooled.missing || here.invalid != pooled.invalid ||
                here.sum != pooled.sum) {
                fprintf(stderr, "track %u: decoding on a pool gave other frames\n", t);
                return 1;
            }
            printf("  track %u: %llu frames, %llu missing, %llu invalid\n", t, (unsigned long long)here.frames,
                   (unsigned long long)here.missing, (unsigned long long)here.invalid);
        }
    }
    sb_disc_free(disc);
    sb_pool_free(pool);
    return 0;
}
