package main

import (
	"bytes"
	"compress/gzip"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net/http"
	"path"
	"strconv"
	"strings"
	"sync"

	"jeeves/internal/cloudkit"
)

// compressible lists the bundle types worth gzipping; images and fonts are already compressed.
var compressible = map[string]bool{".html": true, ".js": true, ".css": true, ".svg": true, ".webmanifest": true}

// minCompress skips files so small that gzip framing would outweigh the saving.
const minCompress = 1024

// maxCompress bounds what one cached gzip copy may cost in memory.
const maxCompress = 8 << 20

type packed struct {
	tag  string
	body []byte
}

// gzipCache holds one compressed copy per bundle file version, keyed by name and validator.
var gzipCache sync.Map

// deliver serves one bundle file so browsers keep it and revalidate on every load:
// an unchanged bundle costs a 304, and text assets travel gzipped when the client accepts it.
func (g *gateway) deliver(w http.ResponseWriter, r *http.Request, name string, info fs.FileInfo, file io.ReadSeeker) {
	tag := fmt.Sprintf("%x-%x", info.ModTime().UnixNano(), info.Size())
	w.Header().Set("Cache-Control", "private, no-cache")
	if compressible[path.Ext(name)] {
		w.Header().Add("Vary", "Accept-Encoding")
	}
	if accepts(r, "gzip") && compressible[path.Ext(name)] && info.Size() >= minCompress && info.Size() <= maxCompress {
		if body, ok := compressed(name, tag, file); ok {
			w.Header().Set("Content-Encoding", "gzip")
			w.Header().Set("ETag", `"`+tag+`-gz"`)
			r.Header.Del("Range")
			http.ServeContent(w, r, name, info.ModTime(), bytes.NewReader(body))
			return
		}
		if _, err := file.Seek(0, io.SeekStart); err != nil {
			log.Printf("asset read failed for %s (%T)", name, err)
			cloudkit.Error(w, 500, "asset unavailable")
			return
		}
	}
	w.Header().Set("ETag", `"`+tag+`"`)
	http.ServeContent(w, r, name, info.ModTime(), file)
}

// compressed returns the cached gzip body for this file version, building it on first use.
func compressed(name, tag string, file io.Reader) ([]byte, bool) {
	if hit, ok := gzipCache.Load(name); ok && hit.(packed).tag == tag {
		return hit.(packed).body, true
	}
	var out bytes.Buffer
	zw, err := gzip.NewWriterLevel(&out, gzip.BestCompression)
	if err == nil {
		_, err = io.Copy(zw, file)
	}
	if err == nil {
		err = zw.Close()
	}
	if err != nil {
		log.Printf("asset compression failed for %s (%T); serving it uncompressed", name, err)
		return nil, false
	}
	gzipCache.Store(name, packed{tag: tag, body: out.Bytes()})
	return out.Bytes(), true
}

// accepts reports whether the Accept-Encoding header allows the coding, honouring q=0 refusals.
func accepts(r *http.Request, coding string) bool {
	for part := range strings.SplitSeq(r.Header.Get("Accept-Encoding"), ",") {
		name, params, _ := strings.Cut(strings.TrimSpace(part), ";")
		if !strings.EqualFold(strings.TrimSpace(name), coding) {
			continue
		}
		key, value, found := strings.Cut(strings.ReplaceAll(params, " ", ""), "=")
		if !found || !strings.EqualFold(key, "q") {
			return true
		}
		weight, err := strconv.ParseFloat(value, 64)
		return err == nil && weight > 0
	}
	return false
}
