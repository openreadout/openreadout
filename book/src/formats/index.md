# Formats

OpenReadout reads the formats below. Each name links to a page with our notes on the format and a link to the provenance log of its reader. The version you have installed is the authority: `openreadout self formats` lists every reader with its confidence (`high`, `medium` or `low`) and its known gaps.

| Family | Formats |
| --- | --- |
| Light microscopy | [Zeiss CZI](czi.md), [Nikon ND2](nd2.md), [Leica LIF](lif.md), [Olympus/Evident OIR](oir.md), [Olympus/Evident cellSens VSI](vsi.md), [Olympus FluoView OIB/OIF](oif.md), [Zeiss AxioVision ZVI](zvi.md), [TIFF family](tiff.md) (OME-TIFF, ImageJ, Aperio SVS, Hamamatsu NDPI, Zeiss LSM, MetaMorph and others), [OME-Zarr](ome-zarr.md), [Imaris IMS](ims.md), [Hamamatsu DCIMG](dcimg.md), [3DHISTECH MIRAX](mirax.md) |
| [High-content screening](hcs.md) | [Harmony](opera-harmony.md) (Opera Phenix, Operetta), [ImageXpress](imagexpress.md), [Yokogawa CellVoyager](cellvoyager.md) |
| [Electron microscopy](em.md) | [MRC/CCP4](mrc.md), [Gatan DM3/DM4/DM5](dm.md), [FEI TIA SER/EMI](ser.md), [Velox EMD](emd.md) |
| Flow cytometry | [FCS](fcs.md), [FlowJo workspaces and Gating-ML](flowjo-wsp.md) |
| Electrophysiology | [Axon ABF and ATF](abf.md), [Neuralynx](neuralynx.md), [Blackrock NSx/NEV](blackrock.md), [SpikeGLX](spikeglx.md), [Intan RHD/RHS](intan.md), [Plexon PLX/PL2](plexon.md), [HEKA PatchMaster](heka-patchmaster.md), [CED Spike2](ced-spike2.md), [WinWCP](winwcp.md), [Open Ephys](open-ephys.md), [NWB and generic HDF5](hdf5.md) |
| NMR | [Bruker TopSpin](bruker-nmr.md), [Varian/Agilent VnmrJ](varian-nmr.md), [JEOL Delta](jeol-jdf.md), [Magritek Spinsolve](magritek-spinsolve.md), [JCAMP-DX](jcamp-dx.md) |
| IR, Raman and UV-Vis spectroscopy | [Bruker OPUS](bruker-opus.md), [Thermo OMNIC](thermo-omnic.md), [Renishaw WiRE](renishaw-wdf.md), [PerkinElmer .sp](perkinelmer-sp.md), [PerkinElmer Spotlight](perkinelmer-fsm.md), [JASCO Spectra Manager](jasco-jws.md), [Agilent Cary UV-Vis](agilent-cary.md), [Agilent FT-IR imaging](agilent-fpa.md), [Galactic/GRAMS SPC](galactic-spc.md), [WITec Project](witec-project.md) |
| Mass spectrometry | [Thermo RAW](thermo-raw.md), [Agilent MassHunter](agilent-masshunter.md), [Waters MassLynx](waters-raw.md), [Sciex WIFF](sciex-wiff.md), [Bruker timsTOF](bruker-tdf.md), [mzML, mzMLb, imzML and mzXML](mzml.md) |
| Chromatography | [Agilent ChemStation](chemstation.md), [Agilent OpenLab CDS](openlab-cds.md), [AIA/ANDI netCDF](andi-chrom.md), [Shimadzu LabSolutions](shimadzu.md), [Thermo Chromeleon](chromeleon.md), [Waters Empower exports](empower-arw.md) |
| Plate readers and qPCR | [Plate-reader exports](plate-readers.md), [real-time PCR](qpcr.md) (RDML, Applied Biosystems, Rotor-Gene, LightCycler) |
| Bench biophysics | [Cytiva ÄKTA/UNICORN](cytiva-unicorn.md), [MicroCal ITC](microcal-itc.md), [Cytiva Biacore](cytiva-biacore.md), [Agilent Seahorse XF](agilent-seahorse.md), [Sartorius Octet](sartorius-octet.md), [Malvern Zetasizer](malvern-zetasizer.md), [Bio-Rad Image Lab gels](biorad-scn.md), [Molecular Dynamics GEL](tiff.md#molecular-dynamics-gel-typhoon-storm-and-fla-scanners), [GenePix microarrays](genepix-gpr.md) |
| Materials and electrochemistry | [Bruker EPR](bruker-epr.md), [X-ray diffraction](xrd.md), [BioLogic EC-Lab](biologic-eclab.md), [Gamry Framework](gamry-dta.md), [Neware BTS](neware.md), [Arbin MITS Pro](arbin-res.md), [NETZSCH Proteus](netzsch-ngb.md), [TA Instruments Universal Analysis](ta-universal-analysis.md), [TA Instruments TRIOS](ta-trios.md) |

OpenReadout never writes these formats. It exports to open formats instead; see [`export`](../reference/commands/export.md).

## What a format page contains

Each page shows our notes on the format from [`docs/formats/`](https://github.com/openreadout/openreadout/tree/main/docs/formats): the byte layout, the metadata OpenReadout normalizes, and a vocabulary table of every public name in the reader's code. It links to the reader's provenance log, which records which files were examined, which prior art was consulted and what was inferred from what. Together they are the evidence behind the [clean-room policy](../project/clean-room.md). If you know something these pages do not, a pull request that adds it, with how you know it, is welcome.

## Requesting a format

Open a [format request](https://github.com/openreadout/openreadout/issues/new/choose). The most useful thing to attach is a link to a public file under an open license, with the name and version of the acquisition software.
