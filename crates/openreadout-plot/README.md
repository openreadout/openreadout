# openreadout-plot

Figures from lab files for OpenReadout. A JSON spec names the data of each series by file and selector (a trace, sweep and channel; a chromatogram or the spectrum at a retention time; plate wells by range or layout group; FCS parameters and gate populations; f–I curves and dose-response fits from the analyses; a table file's columns) or gives a few inline values, and says how to arrange them: panels, small multiples, plate positions, shared and log axes, bands, lines, labelled points and arrows. The crate reads the data through the OpenReadout readers, picks marks, ranges, units and labels from it, draws the figure with `openreadout-figure` and returns it with a summary of what was drawn. Figures render to PNG, SVG and PDF. It backs `openreadout plot` and the MCP tool `openreadout_plot`.

This crate is part of [OpenReadout](https://github.com/openreadout/openreadout), which reads raw lab-instrument files without vendor software. Licensed MIT OR Apache-2.0.
