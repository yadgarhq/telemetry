# telemetry

D67's instrumentation seam: the shared crate every service emits through.

## Three signals, and they are not interchangeable

|                | carries                                       | why it cannot be one of the others                    |
| -------------- | --------------------------------------------- | ----------------------------------------------------- |
| **metric**     | `tool`, `kind`, `outcome` — bounded           | a per-user label is one time series per user per tool |
| **wide event** | plus user, instance, project, exact timestamp | the unbounded dimensions have to live somewhere       |
| **span**       | causality across hops                         | answers _where the time went_                         |

**The cardinality rule is the whole boundary**: an unbounded dimension goes on the
event, never on a metric label. Cardinality is the product of every label's range,
so a per-user label is what makes the metrics store fall over.

## Emission never blocks and never fails a call

This sits on `recall`, the only latency-critical path (D25). A record that cannot
be written is logged and dropped; the call still succeeds. Telemetry that can fail
a request is a new outage source bolted to every path in the system.

That is the exact **opposite** of the capability probe (D69), which must always
fail boot. The two rules point opposite ways on purpose.

## The estimator is not a tokenizer

Vendoring a real BPE tokenizer ships one model's vocabulary: correct for that
model, wrong for every other caller, stale when the vendor changes it.

**Bytes and words are both recorded raw**, because the ratio between them is a
signal in its own right — a high bytes-per-word means an identifier- and
envelope-heavy payload, which calls for trimming the envelope rather than
returning less content. Those are different fixes.

**The estimate takes the larger of two paths**, per class:

```
max( words × tokens_per_word,  bytes ÷ bytes_per_token )
```

Words alone fail silently on dense payloads — a JSON envelope with no whitespace
is one "word" and fifty bytes, and a word-only estimate returns three tokens for
something costing twenty. Under-reporting is the dangerous direction: it makes a
response look cheaper than it is, which is the opposite of what the measurement
is for.

**The coefficients are provisional and labelled as such.** The prose figure
(~1.3 tokens/word) is the well-known English average; the rest are reasoned from
structure. `estimator::error_ratio` exists so calibration replaces them with
evidence — ground truth comes from `ask`, which gets real token counts back from
LLM providers as a side effect of doing its job.

**Every estimate carries `estimator_version`.** Retuning changes what every
historical number means, so without it a dashboard silently compares two months
under different algorithms. Bytes and words stay raw, so a retune never
invalidates the measurement underneath — the estimate is derived and can be
recomputed.

## Transport is boring on purpose

JSON to stdout, collected by the cluster. Not an RPC: a telemetry call on every
request would add a hop to the path that must not block, and stdout survives
yadgar being broken in a way a telemetry service does not.
