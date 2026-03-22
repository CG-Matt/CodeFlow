# `.graph` Format

CodeFlow uses `.graph` files to represent control-flow graphs. The file extension is `.graph`, and the content is JSON.

## Top-Level Structure

A graph file is a JSON object with these fields:

- `source` (string): source file path used to generate the graph
- `node_count` (number): number of nodes
- `edge_count` (number): number of edges
- `nodes` (array): list of node objects
- `edges` (array): list of edge objects
- `start` (string): node id of the entry node
- `end` (string): node id of the exit node

## Node Object

Each item in `nodes` has:

- `id` (string): unique node id, usually `n1`, `n2`, ...
- `shape` (string): one of common flowchart types such as `terminator`, `process`, `decision`, `loop`
- `text` (string): label shown in the node
- `lane` (number): horizontal lane index used by layout
- `line_start` (number or null): source line where this block starts
- `line_end` (number or null): source line where this block ends
- `level` (number): vertical level assigned for layout

## Edge Object

Each item in `edges` has:

- `from` (string): source node id
- `to` (string): target node id
- `label` (string or null): branch/flow label (`yes`, `no`, `elseif`, `else`, `enter`, `done`, `next`, etc.)
- `back_edge` (boolean): true for loop-back or upward-flow edges

## Example

```json
{
  "source": "example.py",
  "node_count": 3,
  "edge_count": 2,
  "nodes": [
    {"id": "n1", "shape": "terminator", "text": "Start", "lane": 0, "line_start": 1, "line_end": 1, "level": 0},
    {"id": "n2", "shape": "process", "text": "Assign: x", "lane": 0, "line_start": 2, "line_end": 2, "level": 1},
    {"id": "n3", "shape": "terminator", "text": "End", "lane": 0, "line_start": null, "line_end": null, "level": 2}
  ],
  "edges": [
    {"from": "n1", "to": "n2", "label": null, "back_edge": false},
    {"from": "n2", "to": "n3", "label": null, "back_edge": false}
  ],
  "start": "n1",
  "end": "n3"
}
```

## Notes

- `.graph` is a naming convention; the file is plain JSON.
- Unknown extra fields should be ignored by consumers.
- Renderers rely on `nodes`, `edges`, `start`, and `end`; `node_count` and `edge_count` are metadata.
