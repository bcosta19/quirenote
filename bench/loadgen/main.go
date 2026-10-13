// loadgen: the single load generator used against BOTH servers (Go and Rust). Standard library
// only. Each subcommand prints one JSON object on stdout (the driver aggregates them) and a few
// human lines on stderr.
//
//	loadgen conflict    --url U --vault V --clients N [--size B]          scenario 1 (same version, same note)
//	loadgen rmw         --url U --vault V --workers N --writes K          scenario 1 (read-modify-write, lost updates)
//	loadgen writes      --url U --clients C --duration D [--size B]       scenario 2 (different notes)
//	loadgen reads       --url U --kind note|tree|search|backlinks --index F --clients C --duration D   scenario 3
//	loadgen sse         --url U --vault V --pid P --clients K --events M  scenario 5
//	loadgen consistency --url U --vault V [--n N]                         scenario 6
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"math/rand/v2"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: loadgen <conflict|rmw|writes|reads|sse|consistency> [flags]")
		os.Exit(2)
	}
	cmd, args := os.Args[1], os.Args[2:]
	var out any
	switch cmd {
	case "conflict":
		out = cmdConflict(args)
	case "rmw":
		out = cmdRMW(args)
	case "writes":
		out = cmdWrites(args)
	case "reads":
		out = cmdReads(args)
	case "sse":
		out = cmdSSE(args)
	case "consistency":
		out = cmdConsistency(args)
	default:
		fmt.Fprintln(os.Stderr, "unknown command", cmd)
		os.Exit(2)
	}
	b, _ := json.Marshal(out)
	fmt.Println(string(b))
}

// ---------------------------------------------------------------------------------------------
// HTTP plumbing

func newClient(conns int) *http.Client {
	tr := &http.Transport{
		MaxIdleConns:        conns + 16,
		MaxIdleConnsPerHost: conns + 16,
		MaxConnsPerHost:     0,
		IdleConnTimeout:     30 * time.Second,
		DisableCompression:  true,
		DialContext:         (&net.Dialer{Timeout: 10 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
	}
	return &http.Client{Transport: tr, Timeout: 60 * time.Second}
}

type note struct {
	Path    string `json:"path"`
	Content string `json:"content"`
	Mtime   string `json:"mtime"`
}

// do sends a JSON request; returns status, body, error (transport errors are NOT HTTP statuses).
func do(c *http.Client, method, url string, body any) (int, []byte, error) {
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, url, rd)
	if err != nil {
		return 0, nil, err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.Do(req)
	if err != nil {
		return 0, nil, err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	return resp.StatusCode, data, err
}

func mustNote(c *http.Client, base, p string) note {
	st, b, err := do(c, "GET", base+"/api/notes/"+p, nil)
	if err != nil || st != 200 {
		fatal("GET %s: %d %v", p, st, err)
	}
	var n note
	_ = json.Unmarshal(b, &n)
	return n
}

func fatal(f string, a ...any) {
	fmt.Fprintf(os.Stderr, "loadgen: "+f+"\n", a...)
	os.Exit(1)
}

func pct(sorted []time.Duration, p float64) float64 {
	if len(sorted) == 0 {
		return 0
	}
	i := int(float64(len(sorted)-1) * p)
	return float64(sorted[i].Microseconds()) / 1000
}

type latStats struct {
	N      int     `json:"n"`
	P50ms  float64 `json:"p50_ms"`
	P95ms  float64 `json:"p95_ms"`
	P99ms  float64 `json:"p99_ms"`
	MaxMs  float64 `json:"max_ms"`
	MeanMs float64 `json:"mean_ms"`
}

func stats(d []time.Duration) latStats {
	sort.Slice(d, func(i, j int) bool { return d[i] < d[j] })
	var sum time.Duration
	for _, x := range d {
		sum += x
	}
	s := latStats{N: len(d), P50ms: pct(d, .5), P95ms: pct(d, .95), P99ms: pct(d, .99)}
	if len(d) > 0 {
		s.MaxMs = float64(d[len(d)-1].Microseconds()) / 1000
		s.MeanMs = float64(sum.Microseconds()) / 1000 / float64(len(d))
	}
	return s
}

func filler(client, size int) string {
	head := fmt.Sprintf("client %d\n", client)
	if size <= len(head) {
		return head
	}
	// content that identifies the writer in EVERY 1 KiB block, so an interleaving of two writers
	// would be detectable (not just a truncation)
	var b strings.Builder
	b.WriteString(head)
	line := fmt.Sprintf("c%d:%s\n", client, strings.Repeat("x", 1000))
	for b.Len() < size {
		b.WriteString(line)
	}
	return b.String()[:size]
}

func leftoverTmp(dir string) []string {
	var out []string
	_ = filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() && strings.HasSuffix(d.Name(), ".tmp") {
			out = append(out, p)
		}
		return nil
	})
	return out
}

// ---------------------------------------------------------------------------------------------
// scenario 1a: N clients, all holding the SAME version, all save at once

func cmdConflict(args []string) any {
	fs := flag.NewFlagSet("conflict", flag.ExitOnError)
	url := fs.String("url", "", "")
	vault := fs.String("vault", "", "")
	clients := fs.Int("clients", 50, "")
	size := fs.Int("size", 256<<10, "bytes of content per client")
	_ = fs.Parse(args)
	c := newClient(*clients)
	name := fmt.Sprintf("bench/conflict-%d.md", *clients)
	if st, _, err := do(c, "PUT", *url+"/api/notes/"+name, map[string]string{"content": "base\n"}); err != nil || st != 200 {
		fatal("seed: %d %v", st, err)
	}
	base := mustNote(c, *url, name)
	contents := make([]string, *clients)
	for i := range contents {
		contents[i] = filler(i, *size)
	}
	status := make([]int, *clients)
	errs := make([]error, *clients)
	var ready, wg sync.WaitGroup
	start := make(chan struct{})
	ready.Add(*clients)
	wg.Add(*clients)
	lat := make([]time.Duration, *clients)
	for i := 0; i < *clients; i++ {
		go func() {
			defer wg.Done()
			ready.Done()
			<-start
			t := time.Now()
			status[i], _, errs[i] = do(c, "PUT", *url+"/api/notes/"+name, map[string]string{"content": contents[i], "mtime": base.Mtime})
			lat[i] = time.Since(t)
		}()
	}
	ready.Wait()
	t0 := time.Now()
	close(start)
	wg.Wait()
	wall := time.Since(t0)

	accepted, conflicts, other, transport := []int{}, 0, map[int]int{}, 0
	for i := range status {
		switch {
		case errs[i] != nil:
			transport++
		case status[i] == 200:
			accepted = append(accepted, i)
		case status[i] == 409:
			conflicts++
		default:
			other[status[i]]++
		}
	}
	disk, _ := os.ReadFile(filepath.Join(*vault, name))
	integrity := "n/a"
	winner := -1
	for i := range contents {
		if string(disk) == contents[i] {
			winner = i
		}
	}
	switch {
	case winner < 0:
		integrity = "CORRUPT: final file is not any client's content (truncated or interleaved)"
	case len(accepted) == 1 && accepted[0] == winner:
		integrity = "ok"
	case len(accepted) > 1:
		integrity = fmt.Sprintf("LOST UPDATE: %d saves accepted for one version, only one survives", len(accepted))
	case len(accepted) == 0:
		integrity = "no save accepted"
	default:
		integrity = "final file differs from the accepted client"
	}
	return map[string]any{
		"scenario": "conflict", "clients": *clients, "size": *size,
		"accepted": len(accepted), "rejected_409": conflicts, "other_status": other, "transport_errors": transport,
		"lost_updates": max(0, len(accepted)-1), "integrity": integrity,
		"leftover_tmp": len(leftoverTmp(filepath.Join(*vault, "bench"))),
		"wall_ms":      float64(wall.Microseconds()) / 1000, "latency": stats(lat),
	}
}

// ---------------------------------------------------------------------------------------------
// scenario 1b: read-modify-write workers on ONE note (each accepted write must survive)

func cmdRMW(args []string) any {
	fs := flag.NewFlagSet("rmw", flag.ExitOnError)
	url := fs.String("url", "", "")
	vault := fs.String("vault", "", "")
	workers := fs.Int("workers", 50, "")
	writes := fs.Int("writes", 10, "accepted writes per worker")
	_ = fs.Parse(args)
	c := newClient(*workers)
	name := fmt.Sprintf("bench/rmw-%d.md", *workers)
	if st, _, err := do(c, "PUT", *url+"/api/notes/"+name, map[string]string{"content": ""}); err != nil || st != 200 {
		fatal("seed: %d %v", st, err)
	}
	var accepted, rejected, errCount atomic.Int64
	var wg sync.WaitGroup
	t0 := time.Now()
	for w := 0; w < *workers; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			done := 0
			for done < *writes {
				st, b, err := do(c, "GET", *url+"/api/notes/"+name, nil)
				if err != nil || st != 200 {
					errCount.Add(1)
					continue
				}
				var n note
				_ = json.Unmarshal(b, &n)
				st, _, err = do(c, "PUT", *url+"/api/notes/"+name, map[string]string{"content": n.Content + fmt.Sprintf("%d:%d\n", w, done), "mtime": n.Mtime})
				switch {
				case err != nil:
					errCount.Add(1)
				case st == 200:
					done++
					accepted.Add(1)
				case st == 409:
					rejected.Add(1)
				default:
					errCount.Add(1)
				}
			}
		}()
	}
	wg.Wait()
	wall := time.Since(t0)
	disk, _ := os.ReadFile(filepath.Join(*vault, name))
	lines := map[string]bool{}
	n := 0
	for _, l := range strings.Split(string(disk), "\n") {
		if l != "" {
			n++
			lines[l] = true
		}
	}
	return map[string]any{
		"scenario": "rmw", "workers": *workers, "writes_each": *writes,
		"accepted": accepted.Load(), "rejected_409": rejected.Load(), "errors": errCount.Load(),
		"lines_on_disk": n, "unique_lines": len(lines), "lost_updates": accepted.Load() - int64(len(lines)),
		"wall_ms": float64(wall.Microseconds()) / 1000, "leftover_tmp": len(leftoverTmp(filepath.Join(*vault, "bench"))),
	}
}

// ---------------------------------------------------------------------------------------------
// scenario 2: concurrent saves of DIFFERENT notes

func cmdWrites(args []string) any {
	fs := flag.NewFlagSet("writes", flag.ExitOnError)
	url := fs.String("url", "", "")
	clients := fs.Int("clients", 64, "")
	dur := fs.Duration("duration", 10*time.Second, "")
	size := fs.Int("size", 4096, "")
	_ = fs.Parse(args)
	c := newClient(*clients)
	mt := make([]string, *clients)
	for i := 0; i < *clients; i++ {
		p := fmt.Sprintf("w/%d/note-%d.md", i%32, i)
		st, b, err := do(c, "PUT", *url+"/api/notes/"+p, map[string]string{"content": "seed"})
		if err != nil || st != 200 {
			fatal("seed %s: %d %v", p, st, err)
		}
		var n note
		_ = json.Unmarshal(b, &n)
		mt[i] = n.Mtime
	}
	var wg sync.WaitGroup
	lats := make([][]time.Duration, *clients)
	var ok, bad, transport atomic.Int64
	deadline := time.Now().Add(*dur)
	t0 := time.Now()
	for i := 0; i < *clients; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			p := fmt.Sprintf("w/%d/note-%d.md", i%32, i)
			k := 0
			for time.Now().Before(deadline) {
				k++
				content := filler(i*100000+k, *size)
				t := time.Now()
				st, b, err := do(c, "PUT", *url+"/api/notes/"+p, map[string]string{"content": content, "mtime": mt[i]})
				d := time.Since(t)
				switch {
				case err != nil:
					transport.Add(1)
				case st == 200:
					ok.Add(1)
					lats[i] = append(lats[i], d)
					var n note
					_ = json.Unmarshal(b, &n)
					mt[i] = n.Mtime
				default:
					bad.Add(1)
				}
			}
		}()
	}
	wg.Wait()
	wall := time.Since(t0)
	var all []time.Duration
	for _, l := range lats {
		all = append(all, l...)
	}
	return map[string]any{
		"scenario": "writes", "clients": *clients, "size": *size, "ok": ok.Load(), "non_200": bad.Load(), "transport_errors": transport.Load(),
		"rps": float64(ok.Load()) / wall.Seconds(), "wall_s": wall.Seconds(), "latency": stats(all),
	}
}

// ---------------------------------------------------------------------------------------------
// scenario 3: reads

func cmdReads(args []string) any {
	fs := flag.NewFlagSet("reads", flag.ExitOnError)
	url := fs.String("url", "", "")
	kind := fs.String("kind", "note", "note|tree|search|backlinks")
	index := fs.String("index", "", "file with one item per line (note paths / queries / note names)")
	clients := fs.Int("clients", 64, "")
	dur := fs.Duration("duration", 10*time.Second, "")
	warm := fs.Duration("warmup", 2*time.Second, "")
	_ = fs.Parse(args)
	var items []string
	if *index != "" {
		b, err := os.ReadFile(*index)
		if err != nil {
			fatal("index: %v", err)
		}
		for _, l := range strings.Split(string(b), "\n") {
			if l != "" {
				items = append(items, l)
			}
		}
	}
	urlFor := func(r *rand.Rand) string {
		switch *kind {
		case "tree":
			return *url + "/api/tree"
		case "search":
			return *url + "/api/search?q=" + queryEscape(items[r.IntN(len(items))])
		case "backlinks":
			return *url + "/api/backlinks?note=" + queryEscape(items[r.IntN(len(items))])
		default:
			return *url + "/api/notes/" + pathEscape(items[r.IntN(len(items))])
		}
	}
	c := newClient(*clients)
	run := func(d time.Duration, record bool) (lat []time.Duration, ok, bad, tr int64, bytesRead int64, wall time.Duration) {
		var wg sync.WaitGroup
		lats := make([][]time.Duration, *clients)
		var okc, badc, trc, byt atomic.Int64
		deadline := time.Now().Add(d)
		t0 := time.Now()
		for i := 0; i < *clients; i++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				r := rand.New(rand.NewPCG(uint64(i)+1, 99))
				for time.Now().Before(deadline) {
					t := time.Now()
					st, b, err := do(c, "GET", urlFor(r), nil)
					dd := time.Since(t)
					switch {
					case err != nil:
						trc.Add(1)
					case st == 200:
						okc.Add(1)
						byt.Add(int64(len(b)))
						if record {
							lats[i] = append(lats[i], dd)
						}
					default:
						badc.Add(1)
					}
				}
			}()
		}
		wg.Wait()
		for _, l := range lats {
			lat = append(lat, l...)
		}
		return lat, okc.Load(), badc.Load(), trc.Load(), byt.Load(), time.Since(t0)
	}
	if *warm > 0 {
		run(*warm, false)
	}
	lat, ok, bad, tr, byt, wall := run(*dur, true)
	return map[string]any{
		"scenario": "reads", "kind": *kind, "clients": *clients, "ok": ok, "non_200": bad, "transport_errors": tr,
		"rps": float64(ok) / wall.Seconds(), "mb_per_s": float64(byt) / wall.Seconds() / 1e6, "avg_resp_bytes": func() int64 {
			if ok == 0 {
				return 0
			}
			return byt / ok
		}(), "latency": stats(lat),
	}
}

func queryEscape(s string) string {
	var b strings.Builder
	for _, c := range []byte(s) {
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_' || c == '.' || c == '~' {
			b.WriteByte(c)
		} else {
			fmt.Fprintf(&b, "%%%02X", c)
		}
	}
	return b.String()
}

func pathEscape(s string) string {
	parts := strings.Split(s, "/")
	for i, p := range parts {
		parts[i] = queryEscape(p)
	}
	return strings.Join(parts, "/")
}

// ---------------------------------------------------------------------------------------------
// scenario 5: SSE fan-out

func procStatus(pid int, key string) int64 {
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/status", pid))
	if err != nil {
		return -1
	}
	for _, l := range strings.Split(string(b), "\n") {
		if strings.HasPrefix(l, key+":") {
			f := strings.Fields(l)
			v, _ := strconv.ParseInt(f[1], 10, 64)
			return v // kB
		}
	}
	return -1
}

func procThreads(pid int) int64 { return procStatus(pid, "Threads") }

func cmdSSE(args []string) any {
	fs := flag.NewFlagSet("sse", flag.ExitOnError)
	url := fs.String("url", "", "")
	vault := fs.String("vault", "", "")
	pid := fs.Int("pid", 0, "server pid (for RSS)")
	clients := fs.Int("clients", 500, "")
	events := fs.Int("events", 20, "")
	_ = fs.Parse(args)
	hostport := strings.TrimPrefix(*url, "http://")

	type ev struct {
		mu   sync.Mutex
		recv int
		last time.Time
	}
	evs := make([]*ev, *events)
	idx := map[string]int{}
	for i := range evs {
		evs[i] = &ev{}
		idx[fmt.Sprintf("sse-%d.md", i)] = i
	}
	var connected atomic.Int64
	var failed atomic.Int64
	rssBefore := procStatus(*pid, "VmRSS")
	thrBefore := procThreads(*pid)
	conns := make([]net.Conn, 0, *clients)
	var cmu sync.Mutex
	var wg sync.WaitGroup
	sem := make(chan struct{}, 64)
	for i := 0; i < *clients; i++ {
		wg.Add(1)
		sem <- struct{}{}
		go func() {
			defer wg.Done()
			defer func() { <-sem }()
			cn, err := net.DialTimeout("tcp", hostport, 10*time.Second)
			if err != nil {
				failed.Add(1)
				return
			}
			_, _ = fmt.Fprintf(cn, "GET /api/events HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n")
			cmu.Lock()
			conns = append(conns, cn)
			cmu.Unlock()
			go func() {
				rd := bufio.NewReaderSize(cn, 4096)
				announced := false
				for {
					line, err := rd.ReadString('\n')
					if err != nil {
						return
					}
					if !announced && strings.HasPrefix(line, ": connected") {
						announced = true
						connected.Add(1)
					}
					if strings.HasPrefix(line, "data: ") {
						var e struct{ Path string }
						_ = json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &e)
						if k, ok := idx[e.Path]; ok {
							now := time.Now()
							evs[k].mu.Lock()
							evs[k].recv++
							evs[k].last = now
							evs[k].mu.Unlock()
						}
					}
				}
			}()
		}()
	}
	wg.Wait()
	deadline := time.Now().Add(15 * time.Second)
	for connected.Load() < int64(*clients)-failed.Load() && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	time.Sleep(500 * time.Millisecond)
	rssConnected := procStatus(*pid, "VmRSS")
	thrConnected := procThreads(*pid)
	nconn := connected.Load()

	var lats []time.Duration
	missed := 0
	partial := 0
	for i := 0; i < *events; i++ {
		tmp := filepath.Join(*vault, fmt.Sprintf(".sse-%d.tmp", i))
		_ = os.WriteFile(tmp, []byte("# sse\n"), 0o644)
		t0 := time.Now()
		_ = os.Rename(tmp, filepath.Join(*vault, fmt.Sprintf("sse-%d.md", i)))
		end := time.Now().Add(5 * time.Second)
		for time.Now().Before(end) {
			evs[i].mu.Lock()
			r := evs[i].recv
			evs[i].mu.Unlock()
			if int64(r) >= nconn {
				break
			}
			time.Sleep(time.Millisecond)
		}
		evs[i].mu.Lock()
		r, last := evs[i].recv, evs[i].last
		evs[i].mu.Unlock()
		if int64(r) >= nconn {
			lats = append(lats, last.Sub(t0))
		} else if r == 0 {
			missed++
		} else {
			partial++
		}
		time.Sleep(300 * time.Millisecond)
	}
	rssAfter := procStatus(*pid, "VmRSS")
	hwm := procStatus(*pid, "VmHWM")
	cmu.Lock()
	for _, cn := range conns {
		_ = cn.Close()
	}
	cmu.Unlock()
	time.Sleep(time.Second)
	rssClosed := procStatus(*pid, "VmRSS")
	perConn := 0.0
	if nconn > 0 && rssBefore >= 0 {
		perConn = float64(rssConnected-rssBefore) * 1024 / float64(nconn)
	}
	return map[string]any{
		"scenario": "sse", "clients_requested": *clients, "connected": nconn, "connect_failed": failed.Load(),
		"events": *events, "events_all_delivered": len(lats), "events_partial": partial, "events_missed": missed,
		"latency_until_all_received": stats(lats),
		"rss_kb_before":              rssBefore, "rss_kb_connected": rssConnected, "rss_kb_after_events": rssAfter, "rss_kb_after_close": rssClosed,
		"vmhwm_kb": hwm, "bytes_per_connection": perConn, "threads_before": thrBefore, "threads_connected": thrConnected,
	}
}

// ---------------------------------------------------------------------------------------------
// scenario 6: trash / move / relink correctness under concurrency

type check struct {
	Name   string `json:"name"`
	OK     bool   `json:"ok"`
	Detail string `json:"detail,omitempty"`
}

func cmdConsistency(args []string) any {
	fs := flag.NewFlagSet("consistency", flag.ExitOnError)
	url := fs.String("url", "", "")
	vault := fs.String("vault", "", "")
	n := fs.Int("n", 400, "number of notes in the trash / relink tests")
	conc := fs.Int("concurrency", 100, "")
	_ = fs.Parse(args)
	c := newClient(*conc * 2)
	var checks []check
	add := func(name string, ok bool, detail string, a ...any) {
		checks = append(checks, check{name, ok, fmt.Sprintf(detail, a...)})
		fmt.Fprintf(os.Stderr, "%-4v %s %s\n", ok, name, fmt.Sprintf(detail, a...))
	}
	post := func(path string, body any) (int, []byte, error) { return do(c, "POST", *url+path, body) }
	pool := func(items int, f func(i int)) {
		var wg sync.WaitGroup
		sem := make(chan struct{}, *conc)
		for i := 0; i < items; i++ {
			wg.Add(1)
			sem <- struct{}{}
			go func() { defer wg.Done(); defer func() { <-sem }(); f(i) }()
		}
		wg.Wait()
	}

	// (a) N notes with the SAME name, trashed concurrently: every version must survive
	pool(*n, func(i int) {
		post(fmt.Sprintf("/api/notes/ctrash/%d/note.md", i), map[string]string{"content": fmt.Sprintf("version %d", i)})
	})
	paths := make([]string, *n)
	codes := make([]int, *n)
	pool(*n, func(i int) {
		st, b, _ := post("/api/trash", map[string]string{"path": fmt.Sprintf("ctrash/%d/note.md", i)})
		codes[i] = st
		var r struct{ Path string }
		_ = json.Unmarshal(b, &r)
		paths[i] = r.Path
	})
	okc, distinct, intact := 0, map[string]bool{}, 0
	for i := range paths {
		if codes[i] == 200 {
			okc++
			distinct[paths[i]] = true
			if d, err := os.ReadFile(filepath.Join(*vault, paths[i])); err == nil && string(d) == fmt.Sprintf("version %d", i) {
				intact++
			}
		}
	}
	add("trash: same-named notes trashed concurrently all succeed", okc == *n, "%d/%d ok", okc, *n)
	add("trash: every destination distinct (no version overwritten)", len(distinct) == *n, "%d distinct of %d", len(distinct), *n)
	add("trash: every version intact on disk", intact == *n, "%d/%d intact", intact, *n)

	// (b) M different sources moved to ONE destination: exactly one wins
	m := 200
	pool(m, func(i int) {
		post(fmt.Sprintf("/api/notes/cmove/src%d.md", i), map[string]string{"content": fmt.Sprintf("s%d", i)})
	})
	mc := make([]int, m)
	pool(m, func(i int) {
		mc[i], _, _ = post("/api/move", map[string]string{"from": fmt.Sprintf("cmove/src%d.md", i), "to": "cmove/dest.md"})
	})
	won, conflict, otherSt := 0, 0, 0
	for _, s := range mc {
		switch s {
		case 200:
			won++
		case 409:
			conflict++
		default:
			otherSt++
		}
	}
	left := 0
	ents, _ := os.ReadDir(filepath.Join(*vault, "cmove"))
	for _, e := range ents {
		if strings.HasPrefix(e.Name(), "src") {
			left++
		}
	}
	add("move: many sources onto one destination: exactly one 200", won == 1 && conflict == m-1 && otherSt == 0, "200=%d 409=%d other=%d", won, conflict, otherSt)
	add("move: losing sources left in place", left == m-1, "%d sources remain of %d expected", left, m-1)

	// (c) create vs move onto the same destination
	rounds, silent := 200, 0
	for r := 0; r < rounds; r++ {
		post(fmt.Sprintf("/api/notes/cvm/src%d.md", r), map[string]string{"content": "moved"})
		var sc, sm int
		var wg sync.WaitGroup
		gate := make(chan struct{})
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-gate
			sc, _, _ = post(fmt.Sprintf("/api/notes/cvm/dest%d.md", r), map[string]string{"content": "created"})
		}()
		go func() {
			defer wg.Done()
			<-gate
			sm, _, _ = post("/api/move", map[string]string{"from": fmt.Sprintf("cvm/src%d.md", r), "to": fmt.Sprintf("cvm/dest%d.md", r)})
		}()
		close(gate)
		wg.Wait()
		if sc == 201 && sm == 200 {
			silent++
		}
	}
	add("create vs move onto one destination: never both succeed", silent == 0, "%d of %d rounds silently destroyed a file", silent, rounds)

	// (d) relink while other clients keep editing the referencing notes
	refs := *n / 2
	pool(refs, func(i int) {
		post(fmt.Sprintf("/api/notes/relk/ref%d.md", i), map[string]string{"content": fmt.Sprintf("ref %d\n[[Old name]]\n", i)})
	})
	post("/api/notes/relk/Old name.md", map[string]string{"content": "# old\n"})
	post("/api/move", map[string]string{"from": "relk/Old name.md", "to": "relk/New name.md"})
	var stop atomic.Bool
	var accepted atomic.Int64
	tokens := make([]sync.Map, refs)
	var wg sync.WaitGroup
	for w := 0; w < 16; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			r := rand.New(rand.NewPCG(uint64(w)+7, 3))
			k := 0
			for !stop.Load() {
				i := r.IntN(refs)
				p := fmt.Sprintf("relk/ref%d.md", i)
				st, b, err := do(c, "GET", *url+"/api/notes/"+p, nil)
				if err != nil || st != 200 {
					continue
				}
				var nt note
				_ = json.Unmarshal(b, &nt)
				k++
				tok := fmt.Sprintf("edit-%d-%d", w, k)
				st, _, err = do(c, "PUT", *url+"/api/notes/"+p, map[string]string{"content": nt.Content + tok + "\n", "mtime": nt.Mtime})
				if err == nil && st == 200 {
					accepted.Add(1)
					tokens[i].Store(tok, true)
				}
			}
		}()
	}
	time.Sleep(300 * time.Millisecond)
	st, rb, _ := post("/api/relink", map[string]string{"from": "relk/Old name.md", "to": "relk/New name.md"})
	time.Sleep(300 * time.Millisecond)
	stop.Store(true)
	wg.Wait()
	var rr struct{ Updated, Skipped []string }
	_ = json.Unmarshal(rb, &rr)
	lost := 0
	for i := 0; i < refs; i++ {
		d, _ := os.ReadFile(filepath.Join(*vault, fmt.Sprintf("relk/ref%d.md", i)))
		tokens[i].Range(func(k, _ any) bool {
			if !strings.Contains(string(d), k.(string)) {
				lost++
			}
			return true
		})
	}
	add("relink: ran while editors were saving", st == 200, "status %d, updated=%d skipped=%d", st, len(rr.Updated), len(rr.Skipped))
	add("relink: no accepted edit lost (relink never overwrites a newer save)", lost == 0, "%d accepted edits missing of %d", lost, accepted.Load())
	// every ref must end up with the link either rewritten or skipped-but-untouched; none may keep an
	// UNSKIPPED stale link
	stale := 0
	skipped := map[string]bool{}
	for _, s := range rr.Skipped {
		skipped[s] = true
	}
	for i := 0; i < refs; i++ {
		p := fmt.Sprintf("relk/ref%d.md", i)
		d, _ := os.ReadFile(filepath.Join(*vault, p))
		if strings.Contains(string(d), "[[Old name]]") && !skipped[p] {
			stale++
		}
	}
	add("relink: no note left with the old link unless reported as skipped", stale == 0, "%d stale", stale)
	add("no temporary files left in the vault", len(leftoverTmp(*vault)) == 0, "%v", leftoverTmp(*vault))
	return map[string]any{"scenario": "consistency", "checks": checks}
}
