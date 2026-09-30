# Out of Your Head: A Short History of Crossfeed

Put on a pair of headphones and play one of the early stereo Beatles records. The guitar lives entirely in your left ear. The voice lives entirely in your right. Nothing exists in the middle except a strange, hollow absence — as though the band set up inside your skull and forgot to leave room for you.

Nobody records like that anymore, but the feeling never quite went away. Even on a modern, beautifully mixed album, headphone listening has a quality that speaker listening doesn't: the soundstage collapses to a line drawn between your ears, wide but flat, intimate but somehow airless. Audiophiles have a name for the fix, and it is older than most of the headphones it runs on.

## The problem, stated plainly

When you listen to speakers, each ear hears *both* speakers. The left ear gets the left speaker first and loudest, then the right speaker a fraction of a millisecond later and a little quieter, its treble softened by the shadow of your own head. Your brain has spent your whole life decoding those tiny differences of timing and tone into a sense of space. That decoding is why a pair of boxes in a room can conjure a stage.

Headphones bypass all of it. The left channel goes to the left ear and stops there; the right ear hears nothing of it. The brain receives a signal with the spatial information stripped out and does its best anyway, which is why everything feels close, and lateral, and after an hour or two, faintly tiring. The fix is almost embarrassingly simple: mix a little of each channel into the other, soften and delay the borrowed signal the way a head would, and let the brain do what it already knows how to do.

## 1961: Bauer writes the prescription

The first person to formalize this was Benjamin Bauer, whose 1961 paper in the *Journal of the Audio Engineering Society* — "Stereophonic Earphones and Binaural Loudspeakers" — worked out what a headphone feed would need to resemble a loudspeaker feed. His circuit used resistors, inductors, and capacitors to blend the channels with frequency-dependent delay: the electrical equivalent of putting a head between the speakers and the ears.

It was a solution slightly ahead of its problem. In 1961 headphones were a niche — studio tools and late-night compromises, not the primary way anyone heard music. Bauer's paper sat in the literature like a answered question nobody had asked yet. It would take a decade, and a different kind of author, to make crossfeed something you could build on a weekend.

## 1971: Linkwitz publishes the circuit

Siegfried Linkwitz is remembered today for the Linkwitz-Riley crossover and his open-baffle speaker designs, but in December 1971 he published something smaller in *Audio* magazine: "Improved Headphone Listening," complete with a build-it-yourself stereo crossfeed circuit. His description of the problem remains the best one ever written: the circuit, he said, "reduces the unnatural spaciousness of sound reproduction and the complete separation between channels which does not correspond to our normal hearing experience. This 'super stereo' effect, while impressive at first, becomes very tiring after a while."

Linkwitz's design proved to have unusual staying power. As one modern maker of headphone amplifiers puts it, his 1971 circuit "is the basis for many crossfeed designs still in use today." It became the folk standard — the version people built, modified, and argued about for the next thirty years.

## The workbench years

Crossfeed's middle history belongs to the workbench. In the late 1990s and early 2000s, the DIY headphone scene gathered around sites like HeadWize, where builders shared amplifier circuits the way cooks share recipes. Pow Chu Moy published a crossfeed circuit that he never claimed was a new design — it was, openly, his tweak of Linkwitz's, adjusted by ear and by measurement. Jan Meier, the Dutch engineer behind Corda headphone amplifiers, developed his own "natural crossfeed" and published the circuit for anyone to build, then shipped it in his commercial amps. HeadRoom, the American company that did as much as anyone to make headphone listening a serious pursuit, put crossfeed in its amplifiers and wrote some of the clearest explanations of headphone imaging ever published for a general audience.

Notice what was happening: nobody owned the idea. Bauer's math, Linkwitz's circuit, Moy's tweaks, Meier's voicing — each generation treated the last one's work as a starting point. Crossfeed accumulated like a folk tune, with verses added by whoever picked it up.

## Software eats the soldering iron

The folk process went digital in 2009, when Boris Mikhaylov released version 2.0.0 of his BS2B ("Bauer stereophonic-to-binaural") VST plugin, built on his free DSP library. BS2B did in code what the circuits had done in copper: a low-passed, delayed bleed of each channel into the other. And it did something the circuits never had — it froze three historical recipes as named presets. *Default* (700 Hz, 4.5 dB of feed) approximates virtual speakers at 30 degrees. *Chu Moy* (700 Hz, 6.0 dB) is Moy's hotter tweak. *Jan Meier* (650 Hz, 9.5 dB) is Meier's Corda voicing. Three chapters of the history, selectable from a dropdown.

From there crossfeed became infrastructure. It shipped as a component for foobar2000, as a DSP option in Roon, as a three-preset panel (including Meier) in Neutron Music Player, as a plugin in EasyEffects on Linux. The soldering iron retired; the idea didn't change.

There was also a pro-audio detour worth noting. SPL's Phonitor, a fixture in mastering studios, parameterized crossfeed differently — not as cutoff frequency and feed level but as virtual speaker angle (20 to 55 degrees, 30 the recommended start) and center level. Same physics, different knobs: the engineer's vocabulary instead of the hobbyist's.

## What it can and cannot do

It helps to be honest about the mechanism, because crossfeed's modesty is the point. What it restores are the two coarsest cues your brain uses to place sounds: the *timing* difference between the ears and the *level* difference, the far ear hearing a slightly later, slightly duller copy. Those two cues are largely what tell you "left" from "right," and crossfeed fakes them well enough that hard-panned instruments stop feeling nailed to your temples.

What it does not restore is the fine spectral fingerprint your outer ears impress on sound — the direction-dependent notches and peaks that tell you "in front" from "behind" and "above" from "below." That fingerprint is unique to your ears, which is why true binaural virtualization needs personalized measurements to be fully convincing. Crossfeed doesn't try. Listeners who use it tend to describe the same modest, consistent effects: less fatigue on long sessions, a stage that moves slightly forward and outward, hard-panned early-stereo recordings made listenable. Nobody credible claims it turns headphones into speakers. It turns headphones into slightly more civilized headphones, and for many listeners that is exactly enough.

## Four presets, sixty years

Which brings us to the dropdown menu. There is something quietly pleasing about the way this particular piece of history ended up: the standard software crossfeed offers you Bauer, Chu Moy, and Jan Meier as named choices, and the well-informed implementations add Linkwitz — the 1971 circuit that Moy himself was modifying. Select them in chronological order and you are walking the idea's lineage: the 1961 paper, the 1971 circuit, the workbench tweak, the commercial voicing.

Kahawai's crossfeed stage follows that lineage deliberately. Four presets, each one a chapter: Bauer's original as digitized by BS2B, Moy's hotter tweak of Linkwitz, Meier's Corda voicing, and Linkwitz's own circuit, digitized from the published 1971 design — the ancestor the others descend from. A custom mode sits alongside for anyone who wants to tune by ear, which is, after all, how every one of these recipes came to exist.

Sixty years after Bauer did the math, the prescription hasn't changed: borrow a little from the other side, soften it the way a head would, and let the brain do the rest. The tools got smaller. The ears didn't.

---

### Reference

**Preset parameters** (BS2B published values, as shipped in EasyEffects, Neutron, Roon, and others):

| Preset | Cutoff | Feed |
|---|---|---|
| Bauer (default) | 700 Hz | 4.5 dB |
| Chu Moy | 700 Hz | 6.0 dB |
| Jan Meier | 650 Hz | 9.5 dB |
| Linkwitz | digitized from the 1971 circuit | — |

**Sources and further reading:** Benjamin Bauer, "Stereophonic Earphones and Binaural Loudspeakers," *JAES* (1961); Siegfried Linkwitz, "Improved Headphone Listening," *Audio* (December 1971), with circuit and response curves at linkwitzlab.com/headphone-xfeed.htm; the BS2B library and its release notes at bs2b.sourceforge.net; preset documentation in the EasyEffects crossfeed plugin docs; Jan Meier's published Corda crossfeed notes; SPL Phonitor documentation (speaker-angle parameterization).
