- Attaching many children to one parent is much faster. In the bundled benchmark, attaching
  10,000 children one by one went from 750 ms to about 15 ms. `Children` is now edited in place,
  and checked in constant time.
