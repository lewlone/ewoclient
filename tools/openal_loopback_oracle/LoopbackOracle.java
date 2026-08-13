import com.mojang.blaze3d.audio.Channel;
import com.mojang.blaze3d.audio.Listener;
import com.mojang.blaze3d.audio.ListenerTransform;
import com.mojang.blaze3d.audio.OpenAlUtil;
import com.mojang.blaze3d.audio.SoundBuffer;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.nio.IntBuffer;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import javax.sound.sampled.AudioFormat;
import net.minecraft.world.phys.Vec3;
import org.lwjgl.openal.AL;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.ALC;
import org.lwjgl.openal.ALC10;
import org.lwjgl.openal.ALC11;
import org.lwjgl.openal.ALCCapabilities;
import org.lwjgl.openal.ALCapabilities;
import org.lwjgl.openal.SOFTLoopback;
import org.lwjgl.openal.SOFTSourceResampler;
import org.lwjgl.system.MemoryUtil;

/// Measures what OpenAL Soft does that Minecraft never writes down, so Rewo's
/// mixer can state its divergence as a number.
///
///     pwsh tools/openal_loopback_oracle/run.ps1
///
/// WHY THIS EXISTS. `Channel.java:88-121` is the complete source surface
/// vanilla touches and `Listener.java:14-15` is the complete listener surface.
/// Neither computes a pan, a gain curve or an interpolation - all three live
/// inside `OpenAL.dll`, which is in no decompile this project holds. So
/// `crates/rewo-audio/src/mixer.rs` declares its pan law and its resampler as
/// *stated approximations*, and until something measured them "approximation"
/// was a word rather than a number. This drives the real DLL through an
/// `ALC_SOFT_loopback` device and prints the numbers, in the
/// `tools/java_tostring_oracle/` shape (M114/M125): the JVM runs here, the
/// **vectors** are checked in, and no gate needs a JVM.
///
/// The product is a divergence, not a pass. Rows are consumed by tests in
/// `crates/rewo-audio/src/mixer.rs` that assert a *bound* with the measured
/// number in the assertion message (M12's `nextGaussian`-ULP precedent).
///
/// Every row carries its whole stimulus, so this file is the single description
/// of each experiment and the consumer is one loop over it rather than a second
/// transcription that can drift.
///
/// ASCII ONLY, deliberately. An earlier revision carried em dashes, and a
/// throwaway PowerShell `Set-Content` re-encoded them into mojibake and broke
/// the build - the file-normalisation hazard `REWO_PLAN.md` s0.0 gotcha 9
/// records, reached through a one-line helper script rather than an editor.
///
/// ## THE ONE PLACE THIS IS NOT VANILLA'S CODE
///
/// `Library.init` cannot be pointed at a loopback device. It has one entry
/// point, `init(String, DeviceList, boolean)`, and it opens the device *inside
/// itself*: `Library.java:63` calls `openDeviceOrFallback`, which reaches
/// `ALC10.alcOpenDevice(name)` at `Library.java:231`. There is no overload, no
/// setter and no seam, and `init` is monolithic, so there is no "rest of init"
/// left to run after a reflective poke. `openLoopbackDevice` below therefore
/// **re-implements `Library.init`'s body** (`Library.java:61-120`) with
/// `alcLoopbackOpenDeviceSOFT` substituted for that one call. Everything
/// downstream - `Channel`, `Listener`, `ListenerTransform`, `SoundBuffer`,
/// `OpenAlUtil` - is vanilla's own class, loaded from `26.2.jar`.
///
/// ## WHY THIS CLASS IS *NOT* IN `com.mojang.blaze3d.audio`
///
/// `Channel.create()` is package-private (`Channel.java:23`), so the obvious
/// move is to declare this class in that package - and `26.2.jar` carries no
/// `module-info.class` and declares nothing `Sealed`, which is what makes that
/// look legal. **It is not**, and a check for sealing does not cover it:
/// `26.2.jar` is SIGNED (`META-INF/MOJANGCS.SF`, `META-INF/MOJANGCS.RSA`), and
/// `ClassLoader.checkCerts` refuses to define an *unsigned* class in a package
/// whose other classes are signed. The failure is
///
///     java.lang.SecurityException: class "com.mojang.blaze3d.audio.OpenAlUtil"'s
///     signer information does not match signer information of other classes in
///     the same package
///
/// and it fires on the first *vanilla* class touched rather than on the
/// intruder, so it reads like a corrupt jar. Stripping the signature block
/// would work and would mean grading a jar this project had modified; reaching
/// the one package-private entry point by reflection instead loads every
/// vanilla class **unchanged and still signed**, which is the stronger claim.
/// `Channel.source` needed reflection regardless - it is `private`, so
/// same-package access would not have reached it either.
///
/// ## SIX TRAPS, EACH MEASURED RATHER THAN GUESSED
///
/// 1. **Capture float, never 16-bit.** `ALC_SHORT_SOFT` output is dithered:
///    across three identical renders in one process ~23% of the short bytes
///    differed, while `ALC_FLOAT_SOFT` was bit-exact. A checked-in short vector
///    is flaky *by construction*, at exactly the +-1 LSB magnitude that reads
///    as a rounding disagreement.
/// 2. **Discard the first chunk after every `alSourcePlay`.** OpenAL Soft ramps
///    per-voice gain over the first mixing quantum, so a naive first-chunk peak
///    reads the *previous* voice's gains - which presents as `peakR[i] ==
///    peakL[i-1]` down consecutive rows and looks exactly like a channel swap.
/// 3. **Build the classpath from `26.2.json`, never from a jar glob** - see
///    `run.ps1`.
/// 4. **Witness the stimulus, do not assume it.** Every row carries an FNV-1a
///    hash of the source PCM. The consumer regenerates the stimulus and asserts
///    the hash, so a `Math.sin` / `f64::sin` disagreement of one ULP fails
///    loudly instead of silently comparing two different inputs.
/// 5. **Set the listener explicitly in every stimulus.** OpenAL's untouched
///    listener is `ListenerTransform.INITIAL`, which faces **-Z**, while Rewo's
///    `listener_basis(0, 0)` (`sound_engine.rs:298-304`) faces **+Z** - a half
///    turn apart. A stimulus that omits the listener therefore compares two
///    opposite orientations and "discovers" a left/right inversion that is an
///    artefact of the fixture. The first draft of this file did exactly that.
///    Everything here writes the listener from the same formula Rewo uses.
/// 6. **The context carries state between stimuli.** See `SETTLE_FRAMES`.
///
/// ## PROVENANCE, AND WHY THE HEADER IS NOT DECORATION
///
/// The pan law is selected by `ALC_OUTPUT_MODE_SOFT` and the interpolation by
/// the default resampler, and **both are `alsoft.ini`-overridable**. The header
/// witnesses them rather than assuming them. These numbers are a divergence
/// against **stock OpenAL Soft 1.25.1 as configured on the capturing machine**,
/// not against what every user hears.
public final class LoopbackOracle {
    /// The device rate. Rewo's mixer is constructed at the same number, so the
    /// rate-conversion stimuli are the only ones where source != device.
    static final int RATE = 44100;

    /// Rendered and thrown away after every `play`, for trap 2. One OpenAL Soft
    /// mixing quantum is far smaller than this; the margin is deliberate.
    static final int WARMUP_FRAMES = 4096;

    /// Measured. The DFT reads its head, so the spectral rows need no second
    /// pass.
    ///
    /// A multiple of 100, which is one period of the 441 Hz stimulus at 44100,
    /// so the RMS of the level rows covers a whole number of cycles. At 8192 it
    /// did not, and the resulting 0.04% edge error sat right where the
    /// hard-panned rows are quoted to four figures.
    static final int MEASURE_FRAMES = 8200;

    /// Rendered and discarded BETWEEN stimuli, on top of the warm-up.
    /// Overridable as `args[0]` for tuning; the checked-in capture used this
    /// constant.
    ///
    /// **This is what makes two identical stimuli agree.** The output limiter
    /// is a context-wide gain with a release, so the 32-voice row leaves it
    /// disturbed and it recovers over whatever follows - which presented as two
    /// byte-identical stimuli differing by 3.2%, drifting back toward agreement
    /// further down the file. Measured against the `ctl.posx` pair, the residual
    /// is 3.2% at one second, 0.097% at ten, and 0.0000% from about a minute of
    /// rendered silence onward. Loopback renders far faster than real time, so
    /// a minute per stimulus costs a few seconds of wall clock for the whole
    /// run.
    ///
    /// A silent gap cannot be checked from the inside - the limiter's gain is
    /// invisible while nothing is sounding - so the number is not trusted, it is
    /// witnessed: `ctl.posx.first` and `ctl.posx.last` are the same stimulus at
    /// opposite ends of the run, with the 32-voice row between them, and the
    /// consumer asserts they agree.
    static final int DEFAULT_SETTLE_FRAMES = 2646000;

    static int settleFrames = DEFAULT_SETTLE_FRAMES;

    /// A naive O(n^2) DFT, because the consumer has to reproduce it exactly and
    /// a shared FFT would be a dependency on both sides for a transform that
    /// runs a handful of times.
    static final int DFT_N = 2048;

    /// Bins either side of the fundamental counted as "signal".
    ///
    /// Sized to the window: a 4-term Blackman-Harris main lobe is 8 bins wide,
    /// so 6 clears it with margin. Widening it moves every row identically,
    /// which is why the consumer must use the same number.
    static final int FUND_HALFWIDTH = 6;

    static final int ALC_FREQUENCY = 4103;
    static final int ALC_MONO_SOURCES = 4112;
    static final int ALC_FORMAT_CHANNELS_SOFT = 6544;
    static final int ALC_FORMAT_TYPE_SOFT = 6545;
    static final int ALC_STEREO_SOFT = 5377;
    static final int ALC_FLOAT_SOFT = 5126;
    static final int ALC_HRTF_SOFT = 6546;
    static final int ALC_HRTF_STATUS_SOFT = 6547;
    static final int ALC_NUM_HRTF_SPECIFIERS_SOFT = 6548;
    static final int ALC_HRTF_ID_SOFT = 6550;
    static final int ALC_OUTPUT_LIMITER_SOFT = 6554;
    static final int ALC_OUTPUT_MODE_SOFT = 6572;
    static final int AL_SOURCE_DISTANCE_MODEL = 512;
    static final int AL_BUFFER = 4105;

    /// `disableAttenuation()` rather than `linearAttenuation(max)`.
    static final float NO_ATTENUATION = -1.0f;

    static long device;
    static long context;
    static FloatBuffer scratch;
    static final Listener LISTENER = new Listener();

    public static void main(final String[] args) {
        if (args.length > 0) {
            settleFrames = Integer.parseInt(args[0]);
        }
        openLoopbackDevice();
        scratch = MemoryUtil.memAllocFloat(MEASURE_FRAMES * 2);
        printHeader();
        for (Stim s : stimuli()) {
            run(s);
        }
        System.out.println("# end");
        MemoryUtil.memFree(scratch);
        ALC10.alcDestroyContext(context);
        ALC10.alcCloseDevice(device);
    }

    // ---------------------------------------------------------------- device

    /// `Library.init`'s body (`Library.java:61-120`) with the device call
    /// swapped, plus the three loopback format attributes.
    ///
    /// The attribute list is vanilla's `createAttributes`
    /// (`Library.java:122-133`) verbatim - the HRTF pair guarded on
    /// `alcGetInteger(device, 6548) > 0`, then the limiter ENABLE
    /// unconditionally - with the format triple prepended. The limiter's
    /// *enable* is the one part of the limiter that is in Java; its curve is in
    /// the DLL and is what the `limiter` stimulus measures.
    ///
    /// `useHrtf` is false, matching `Options.java:560-563` where
    /// `directionalAudio` defaults to false, so `createAttributes` writes
    /// `ALC_HRTF_SOFT = 0` explicitly whenever the device advertises any
    /// specifier at all.
    static void openLoopbackDevice() {
        device = SOFTLoopback.nalcLoopbackOpenDeviceSOFT(MemoryUtil.NULL);
        if (device == 0L) {
            throw new IllegalStateException("alcLoopbackOpenDeviceSOFT returned 0");
        }
        ALCCapabilities alcCaps = ALC.createCapabilities(device);
        if (OpenAlUtil.checkALCError(device, "Get capabilities")) {
            throw new IllegalStateException("Failed to get OpenAL capabilities");
        }
        if (!alcCaps.OpenALC11) {
            throw new IllegalStateException("OpenAL 1.1 not supported");
        }
        if (!alcCaps.ALC_SOFT_loopback) {
            throw new IllegalStateException("ALC_SOFT_loopback is not supported");
        }
        if (!SOFTLoopback.alcIsRenderFormatSupportedSOFT(device, RATE, ALC_STEREO_SOFT, ALC_FLOAT_SOFT)) {
            throw new IllegalStateException("stereo/float/" + RATE + " is not a supported render format");
        }

        int numHrtf = ALC10.alcGetInteger(device, ALC_NUM_HRTF_SPECIFIERS_SOFT);
        IntBuffer attr = MemoryUtil.memAllocInt(16);
        attr.put(ALC_FREQUENCY).put(RATE);
        attr.put(ALC_FORMAT_CHANNELS_SOFT).put(ALC_STEREO_SOFT);
        attr.put(ALC_FORMAT_TYPE_SOFT).put(ALC_FLOAT_SOFT);
        if (numHrtf > 0) {
            attr.put(ALC_HRTF_SOFT).put(0);
            attr.put(ALC_HRTF_ID_SOFT).put(0);
        }
        attr.put(ALC_OUTPUT_LIMITER_SOFT).put(1);
        attr.put(0).flip();

        context = ALC10.alcCreateContext(device, attr);
        MemoryUtil.memFree(attr);
        if (OpenAlUtil.checkALCError(device, "Create context") || context == 0L) {
            throw new IllegalStateException("Unable to create OpenAL context");
        }
        ALC10.alcMakeContextCurrent(context);
        ALCapabilities alCaps = AL.createCapabilities(alcCaps);
        OpenAlUtil.checkALError("Initialization");
        if (!alCaps.AL_EXT_source_distance_model) {
            throw new IllegalStateException("AL_EXT_source_distance_model is not supported");
        }
        AL10.alEnable(AL_SOURCE_DISTANCE_MODEL);
        if (!alCaps.AL_EXT_LINEAR_DISTANCE) {
            throw new IllegalStateException("AL_EXT_LINEAR_DISTANCE is not supported");
        }
        OpenAlUtil.checkALError("Enable per-source distance models");
    }

    static void printHeader() {
        p("# OpenAL loopback oracle - generated by tools/openal_loopback_oracle/LoopbackOracle.java");
        p("# Vanilla's own Channel/Listener/SoundBuffer against the real OpenAL.dll.");
        p("# THESE ARE DIVERGENCES, NOT TARGETS. Consumer: crates/rewo-audio/src/mixer.rs.");
        p("# Divergence is against STOCK OpenAL Soft as configured below, not against what");
        p("# every user hears: output mode and default resampler are alsoft.ini-overridable.");
        p("#");
        p("#\tal.version\t" + AL10.alGetString(AL10.AL_VERSION));
        p("#\tal.renderer\t" + AL10.alGetString(AL10.AL_RENDERER));
        p("#\tal.vendor\t" + AL10.alGetString(AL10.AL_VENDOR));
        p("#\talc.device\t" + ALC10.alcGetString(device, ALC11.ALC_ALL_DEVICES_SPECIFIER));
        p("#\trender.rate\t" + RATE);
        p("#\trender.channels\tALC_STEREO_SOFT");
        p("#\trender.type\tALC_FLOAT_SOFT");
        // The pan law's selector. STEREO_BASIC and STEREO_UHJ decode
        // differently; a capture under one is not a capture under the other.
        p("#\talc.output_mode\t" + ALC10.alcGetInteger(device, ALC_OUTPUT_MODE_SOFT)
            + "\t(6574=ALC_STEREO_BASIC_SOFT)");
        p("#\talc.hrtf_status\t" + ALC10.alcGetInteger(device, ALC_HRTF_STATUS_SOFT)
            + "\t(0=ALC_HRTF_DISABLED_SOFT)");
        p("#\talc.num_hrtf_specifiers\t" + ALC10.alcGetInteger(device, ALC_NUM_HRTF_SPECIFIERS_SOFT));
        p("#\talc.output_limiter\t" + ALC10.alcGetInteger(device, ALC_OUTPUT_LIMITER_SOFT));
        // `Library.getChannelCount` (`Library.java:135-190`) reads exactly this
        // attribute, and it is what sizes both channel pools.
        p("#\talc.mono_sources\t" + monoSources());
        int nres = AL10.alGetInteger(SOFTSourceResampler.AL_NUM_RESAMPLERS_SOFT);
        int defres = AL10.alGetInteger(SOFTSourceResampler.AL_DEFAULT_RESAMPLER_SOFT);
        p("#\tal.num_resamplers\t" + nres);
        p("#\tal.default_resampler\t" + defres + "\t"
            + SOFTSourceResampler.alGetStringiSOFT(SOFTSourceResampler.AL_RESAMPLER_NAME_SOFT, defres));
        for (int i = 0; i < nres; i++) {
            p("#\tal.resampler." + i + "\t"
                + SOFTSourceResampler.alGetStringiSOFT(SOFTSourceResampler.AL_RESAMPLER_NAME_SOFT, i));
        }
        p("#\tdft.n\t" + DFT_N);
        p("#\tdft.fund_halfwidth\t" + FUND_HALFWIDTH);
        p("#\tdft.window\tblackman-harris-4");
        p("#\twarmup.frames\t" + WARMUP_FRAMES);
        p("#\tmeasure.frames\t" + MEASURE_FRAMES);
        p("#\tsettle.frames\t" + settleFrames);
        p("#");
        p("#\tCOLUMNS");
        p("#\tid srate frames chans freqL ampL freqR ampR vol pitch rel maxd sx sy sz "
            + "lyaw lpitch lx ly lz voices reslin srchash rmsL rmsR peakL peakR dsrDb fundHz");
        p("#\tmaxd -1 means disableAttenuation(); reslin 1 means AL_SOURCE_RESAMPLER_SOFT forced to Linear;");
        p("#\tfundHz -1 means the distortion statistic is not meaningful for the row and dsrDb is nan.");
        p("#");
    }

    /// `Library.getChannelCount` (`Library.java:135-190`), re-implemented for
    /// the same reason `init` is: it is private.
    static int monoSources() {
        int size = ALC10.alcGetInteger(device, 4098);
        IntBuffer attrs = MemoryUtil.memAllocInt(Math.max(size, 1));
        ALC10.alcGetIntegerv(device, 4099, attrs);
        int result = 30;
        for (int pos = 0; pos + 1 < size; ) {
            int a = attrs.get(pos++);
            if (a == 0) {
                break;
            }
            int v = attrs.get(pos++);
            if (a == ALC_MONO_SOURCES) {
                result = v;
                break;
            }
        }
        MemoryUtil.memFree(attrs);
        return result;
    }

    // -------------------------------------------------------------- stimuli

    /// One experiment, fully described. `freqR`/`ampR` are only read when
    /// `chans == 2`.
    record Stim(String id, int srate, int frames, int chans,
                double freqL, double ampL, double freqR, double ampR,
                float vol, float pitch, boolean rel, float maxd,
                double sx, double sy, double sz,
                float lyaw, float lpitch, double lx, double ly, double lz,
                int voices, boolean resLin, double fundHz) {}

    static Stim mono(String id, float vol, float maxd, double sx, double sy, double sz) {
        return new Stim(id, RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            vol, 1.0f, false, maxd, sx, sy, sz, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0);
    }

    static List<Stim> stimuli() {
        List<Stim> out = new ArrayList<>();

        // (0) The carryover control, and the reason it is a PAIR bracketing
        //     everything: the context carries state between stimuli - the
        //     output limiter's gain most of all - so a row's value can depend
        //     on what ran before it. `ctl.posx.first` and `ctl.posx.last` are
        //     the identical stimulus at opposite ends of the run, and the
        //     consumer asserts they agree. Without them `settleFrames` is a
        //     number someone chose; with them it is a number that is checked.
        out.add(mono("ctl.posx.first", 0.8f, 16.0f, 1.0, 0.0, 0.0));

        // (1) The attenuation curve. The bearing is the same in every row so
        //     the pan divides out. `linear_gain` predicts 1 - d/16 exactly and
        //     exactly zero at the radius, which is the property an
        //     inverse-square model cannot have.
        for (double d : new double[] {0.0, 1.0, 4.0, 8.0, 15.0, 16.0, 64.0}) {
            out.add(mono("dist.d" + fmtId(d), 0.8f, 16.0f, 0.0, 0.0, d));
        }

        // (2) The pan law. Six bearings at unit distance. `posz`/`negz` are the
        //     pair a `dot(direction, right)` pan cannot tell apart, because the
        //     dot product is zero for both - so no curve fitted to the left and
        //     right rows can ever separate them.
        out.add(mono("pan.posx", 0.8f, 16.0f, 1.0, 0.0, 0.0));
        out.add(mono("pan.negx", 0.8f, 16.0f, -1.0, 0.0, 0.0));
        out.add(mono("pan.posz", 0.8f, 16.0f, 0.0, 0.0, 1.0));
        out.add(mono("pan.negz", 0.8f, 16.0f, 0.0, 0.0, -1.0));
        out.add(mono("pan.posy", 0.8f, 16.0f, 0.0, 1.0, 0.0));
        out.add(mono("pan.negy", 0.8f, 16.0f, 0.0, -1.0, 0.0));
        // Off-axis, where an equal-power curve and whatever OpenAL does are
        // least likely to coincide by construction.
        out.add(mono("pan.diag", 0.8f, 16.0f, 0.7071067811865476, 0.0, -0.7071067811865476));

        // (2b) The yaw control. A world-fixed source with the LISTENER turned.
        //      This is what makes the relative rows below interpretable, since
        //      it establishes that turning the listener does move an ordinary
        //      source's image.
        for (float yaw : new float[] {90.0f, 180.0f, 270.0f}) {
            out.add(new Stim("yaw" + fmtId(yaw) + ".posx", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
                0.8f, 1.0f, false, 16.0f, 1.0, 0.0, 0.0, yaw, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0));
        }

        // (3) A pitched listener. `right()` is `forward x up`
        //     (`ListenerTransform.java:8-10`), so the up vector is the only
        //     thing that keeps the image from collapsing at +-90 - where
        //     pinning up to a constant (0,1,0) makes the cross product the ZERO
        //     vector and centres everything.
        for (float lp : new float[] {90.0f, -90.0f, 45.0f}) {
            out.add(new Stim("lpitch" + fmtId(lp) + ".posx", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
                0.8f, 1.0f, false, 16.0f, 1.0, 0.0, 0.0, 0.0f, lp, 0.0, 0.0, 0.0, 1, false, -1.0));
            out.add(new Stim("lpitch" + fmtId(lp) + ".posy", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
                0.8f, 1.0f, false, 16.0f, 0.0, 1.0, 0.0, 0.0f, lp, 0.0, 0.0, 0.0, 1, false, -1.0));
        }

        // (4)/(5)/(6) The resampler. The source tone is a tenth of Nyquist, not
        // a low one: at 480 Hz every interpolator is transparent and the
        // measurement reads its own noise floor whatever the algorithm. The
        // statistic is a distortion-to-signal ratio rather than an RMS
        // difference for the same reason - a tone survives any interpolator at
        // nearly the same RMS.
        //
        // Each row is emitted twice, once with the device default and once with
        // `AL_SOURCE_RESAMPLER_SOFT` forced to Linear, which is Rewo's own
        // algorithm. Both are OpenAL renders of the same source, so everything
        // except the interpolator is held constant and default-minus-linear IS
        // the resampler's own contribution. What remains between the linear row
        // and Rewo is then everything else.
        for (boolean lin : new boolean[] {false, true}) {
            String pre = lin ? "linres." : "";
            // Rate conversion alone: a 48 kHz source into the 44.1 kHz device
            // at pitch 1. The store is genuinely mixed-rate, so this is the
            // common case rather than an edge.
            out.add(new Stim(pre + "rate.48kto44k", 48000, 2 * 48000, 1, 4800.0, 12000.0, 0.0, 0.0,
                0.8f, 1.0f, true, NO_ATTENUATION, 0.0, 0.0, 0.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, lin, 4800.0));
            // Pitch. 2.0 is included and is DEGENERATE on purpose: the step is
            // exactly two source frames, so every output sample lands on a
            // source sample and no interpolator interpolates at all. A
            // comparison using only powers of two would measure zero and
            // conclude the two algorithms agree.
            for (double pch : new double[] {0.5, 0.7, 1.3, 1.5, 2.0}) {
                out.add(new Stim(pre + "pitch.p" + fmtId(pch), RATE, 2 * RATE, 1, 4410.0, 12000.0, 0.0, 0.0,
                    0.8f, (float) pch, true, NO_ATTENUATION, 0.0, 0.0, 0.0,
                    0.0f, 0.0f, 0.0, 0.0, 0.0, 1, lin, 4410.0 * pch));
            }
        }

        // (8) The two non-curve flags.
        //     `noatten.far` puts a source 400 blocks away with attenuation off.
        //     `relative.*` is the sharp one: a mixer that implements
        //     AL_SOURCE_RELATIVE by skipping the listener-position subtraction
        //     and then panning with the listener's current right vector will
        //     agree with OpenAL only when the listener happens to face the
        //     default direction. The yaw-90 and walked rows are what separate
        //     the readings, and `relative.yaw0` is their control.
        out.add(new Stim("noatten.far", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            0.8f, 1.0f, false, NO_ATTENUATION, 0.0, 0.0, -400.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0));
        out.add(new Stim("relative.yaw0", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            0.8f, 1.0f, true, 16.0f, 1.0, 0.0, 0.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0));
        out.add(new Stim("relative.yaw90", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            0.8f, 1.0f, true, 16.0f, 1.0, 0.0, 0.0, 90.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0));
        out.add(new Stim("relative.walked", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            0.8f, 1.0f, true, 16.0f, 1.0, 0.0, 0.0, 0.0f, 0.0f, 100.0, 0.0, 100.0, 1, false, -1.0));

        // (9) A stereo buffer, which settles a claim the plan carries as
        //     `[concurring]` and unverified: that OpenAL does not spatialise a
        //     multi-channel buffer. The two channels carry different tones at
        //     an exact 2:1 amplitude ratio, so a downmix is distinguishable
        //     from a pass-through by more than level. The `d1`/`d8` pair is the
        //     control that says whether ATTENUATION is applied, which is a
        //     separate question from whether the image is panned, and the
        //     `halfvol` row says whether AL_GAIN still applies.
        for (double d : new double[] {1.0, 8.0}) {
            out.add(new Stim("stereo.d" + fmtId(d), RATE, 2 * RATE, 2, 441.0, 12000.0, 1323.0, 6000.0,
                0.8f, 1.0f, false, 16.0f, d, 0.0, 0.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0));
        }
        out.add(new Stim("stereo.halfvol", RATE, 2 * RATE, 2, 441.0, 12000.0, 1323.0, 6000.0,
            0.4f, 1.0f, false, 16.0f, 1.0, 0.0, 0.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, -1.0));
        // The mono twin of `stereo.d1`, so the stereo rows have something in
        // the same scene to be compared against.
        out.add(mono("stereo.monotwin", 0.8f, 16.0f, 1.0, 0.0, 0.0));

        // (7) The limiter, against Rewo's single hard `clamp(-1, 1)`. Coherent
        //     on purpose: it guarantees the sum leaves the representable range,
        //     which is the only regime where the two can differ. The
        //     distortion column is the interesting one - a hard clip squares
        //     the wave off and a limiter does not.
        //
        //     **Placed last** because it is the row that disturbs the context
        //     most, and `limiter.x1` is its own control: same everything, one
        //     voice, so the pair isolates what 32 coherent voices do rather
        //     than what one loud one does.
        out.add(new Stim("limiter.x1", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            1.0f, 1.0f, true, NO_ATTENUATION, 0.0, 0.0, 0.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 1, false, 441.0));
        out.add(new Stim("limiter.x32", RATE, 2 * RATE, 1, 441.0, 12000.0, 0.0, 0.0,
            1.0f, 1.0f, true, NO_ATTENUATION, 0.0, 0.0, 0.0, 0.0f, 0.0f, 0.0, 0.0, 0.0, 32, false, 441.0));

        // (0, closing half) The other end of the carryover bracket, after the
        //     limiter row, so it witnesses recovery from the worst case.
        out.add(mono("ctl.posx.last", 0.8f, 16.0f, 1.0, 0.0, 0.0));

        return out;
    }

    // -------------------------------------------------------------- plumbing

    static void run(Stim s) {
        Src src = s.chans() == 2
            ? stereoTone(s.srate(), s.frames(), s.freqL(), s.ampL(), s.freqR(), s.ampR())
            : tone(s.srate(), s.frames(), s.freqL(), s.ampL());

        setListener(s.lyaw(), s.lpitch(), new Vec3(s.lx(), s.ly(), s.lz()));

        int sharedBuffer = uploadBuffer(src);
        Channel[] chs = new Channel[s.voices()];
        int resLinIdx = s.resLin() ? resamplerIndex("Linear") : -1;
        if (s.resLin() && resLinIdx < 0) {
            throw new IllegalStateException("no resampler named Linear");
        }
        for (int i = 0; i < chs.length; i++) {
            Channel ch = newChannel();
            chs[i] = ch;
            // Vanilla's order is properties, then attach, then play
            // (`SoundEngine.java:417-434`), and `alSourcePlay` before an attach
            // is a no-op - so the order here is not cosmetic.
            ch.setVolume(s.vol());
            ch.setPitch(s.pitch());
            ch.setLooping(true);
            if (s.maxd() == NO_ATTENUATION) {
                ch.disableAttenuation();
            } else {
                ch.linearAttenuation(s.maxd());
            }
            ch.setRelative(s.rel());
            ch.setSelfPosition(new Vec3(s.sx(), s.sy(), s.sz()));
            if (resLinIdx >= 0) {
                AL10.alSourcei(sourceIdOf(ch), SOFTSourceResampler.AL_SOURCE_RESAMPLER_SOFT, resLinIdx);
            }
            // `Channel.attachStaticBuffer` re-uploads per channel; the limiter
            // row wants ONE buffer on 32 sources, so the attach is done with
            // the id `SoundBuffer` already produced. Same AL call
            // (`Channel.java:119-121`), one buffer.
            AL10.alSourcei(sourceIdOf(ch), AL_BUFFER, sharedBuffer);
            ch.play();
        }
        if (OpenAlUtil.checkALError("play " + s.id())) {
            throw new IllegalStateException("AL error while starting " + s.id());
        }

        render(WARMUP_FRAMES);
        float[] out = render(MEASURE_FRAMES);

        double rmsL = rms(out, 0), rmsR = rms(out, 1);
        double peakL = peak(out, 0), peakR = peak(out, 1);
        double dsr = s.fundHz() < 0.0 ? Double.NaN : distortionToSignalDb(out, s.fundHz());
        p(String.format(Locale.ROOT,
            "%s\t%d\t%d\t%d\t%.9g\t%.9g\t%.9g\t%.9g\t%.9g\t%.9g\t%d\t%.9g\t"
                + "%.9g\t%.9g\t%.9g\t%.9g\t%.9g\t%.9g\t%.9g\t%.9g\t%d\t%d\t%d\t"
                + "%.9g\t%.9g\t%.9g\t%.9g\t%s\t%.9g",
            s.id(), s.srate(), s.frames(), s.chans(),
            s.freqL(), s.ampL(), s.freqR(), s.ampR(),
            (double) s.vol(), (double) s.pitch(), s.rel() ? 1 : 0, (double) s.maxd(),
            s.sx(), s.sy(), s.sz(),
            (double) s.lyaw(), (double) s.lpitch(), s.lx(), s.ly(), s.lz(),
            s.voices(), s.resLin() ? 1 : 0, src.hash(),
            rmsL, rmsR, peakL, peakR,
            Double.isNaN(dsr) ? "nan" : String.format(Locale.ROOT, "%.9g", dsr),
            s.fundHz()));

        for (Channel c : chs) {
            c.stop();
            c.destroy();
        }
        AL10.alDeleteBuffers(new int[] {sharedBuffer});
        // Let the context settle before the next stimulus warms up - see
        // SETTLE_FRAMES. Rendered in MEASURE_FRAMES-sized calls because
        // `scratch` is that big.
        for (int done = 0; done < settleFrames; done += MEASURE_FRAMES) {
            render(Math.min(MEASURE_FRAMES, settleFrames - done));
        }
    }

    record Src(byte[] bytes, AudioFormat format, long hash) {}

    /// `s[i] = round(amp * sin(2*pi*f*i/rate))`, little-endian signed 16-bit.
    /// `AudioFormat`'s `bigEndian` is false and
    /// `OpenAlUtil.audioFormatToOpenAl` (`OpenAlUtil.java:55-80`) maps
    /// 1ch/16-bit to `AL_FORMAT_MONO16` and 2ch/16-bit to `AL_FORMAT_STEREO16`.
    static Src tone(int rate, int frames, double freq, double amp) {
        short[] s = new short[frames];
        for (int i = 0; i < frames; i++) {
            s[i] = (short) Math.round(amp * Math.sin(2.0 * Math.PI * freq * i / rate));
        }
        return pack(s, rate, 1);
    }

    static Src stereoTone(int rate, int frames, double fl, double al, double fr, double ar) {
        short[] s = new short[frames * 2];
        for (int i = 0; i < frames; i++) {
            s[i * 2] = (short) Math.round(al * Math.sin(2.0 * Math.PI * fl * i / rate));
            s[i * 2 + 1] = (short) Math.round(ar * Math.sin(2.0 * Math.PI * fr * i / rate));
        }
        return pack(s, rate, 2);
    }

    static Src pack(short[] s, int rate, int channels) {
        ByteBuffer bb = ByteBuffer.allocate(s.length * 2).order(ByteOrder.LITTLE_ENDIAN);
        for (short v : s) {
            bb.putShort(v);
        }
        AudioFormat fmt = new AudioFormat(rate, 16, channels, true, false);
        return new Src(bb.array(), fmt, fnv1a(s));
    }

    /// FNV-1a over the little-endian sample bytes. Trap 4: the consumer
    /// regenerates the stimulus and asserts this, so a one-ULP `sin`
    /// disagreement between the JVM and Rust fails loudly rather than silently
    /// comparing two different inputs.
    static long fnv1a(short[] s) {
        long h = 0xcbf29ce484222325L;
        for (short v : s) {
            h = (h ^ (v & 0xFF)) * 0x100000001b3L;
            h = (h ^ ((v >> 8) & 0xFF)) * 0x100000001b3L;
        }
        return h;
    }

    /// `Channel.create()` (`Channel.java:23-27`) - package-private, reached by
    /// reflection for the signing reason in the class doc. This is vanilla's
    /// own `alGenSources` plus its error check, not a re-implementation.
    static Channel newChannel() {
        try {
            Method m = Channel.class.getDeclaredMethod("create");
            m.setAccessible(true);
            Channel c = (Channel) m.invoke(null);
            if (c == null) {
                throw new IllegalStateException("Channel.create() returned null - source pool exhausted");
            }
            return c;
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("Channel.create() is unreachable", e);
        }
    }

    static int uploadBuffer(Src src) {
        ByteBuffer direct = MemoryUtil.memAlloc(src.bytes().length);
        direct.put(src.bytes()).flip();
        SoundBuffer sb = new SoundBuffer(direct, src.format());
        int id = sb.releaseAlBuffer().orElseThrow(() -> new IllegalStateException("no AL buffer"));
        MemoryUtil.memFree(direct);
        return id;
    }

    /// Writes the listener through vanilla's own `Listener.setTransform`
    /// (`Listener.java:9-16`), from the same basis Rewo builds
    /// (`sound_engine.rs:298-304`) - so the two sides are pointed the same way
    /// by construction rather than by two transcriptions happening to agree.
    /// See trap 5: this is never skipped.
    static void setListener(float yawDeg, float pitchDeg, Vec3 pos) {
        double yaw = Math.toRadians(yawDeg);
        double pitch = Math.toRadians(pitchDeg);
        double sy = Math.sin(yaw), cy = Math.cos(yaw);
        double sp = Math.sin(pitch), cp = Math.cos(pitch);
        Vec3 forward = new Vec3(-cp * sy, -sp, cp * cy);
        Vec3 up = new Vec3(-sp * sy, cp, sp * cy);
        LISTENER.setTransform(new ListenerTransform(pos, forward, up));
        if (OpenAlUtil.checkALError("set listener")) {
            throw new IllegalStateException("AL error while setting the listener");
        }
    }

    /// `Channel.source` (`Channel.java:18`) is private, vanilla never writes
    /// `AL_SOURCE_RESAMPLER_SOFT`, and OpenAL exposes no other route to the
    /// source id.
    static int sourceIdOf(Channel ch) {
        try {
            Field f = Channel.class.getDeclaredField("source");
            f.setAccessible(true);
            return f.getInt(ch);
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("Channel.source is unreachable", e);
        }
    }

    static int resamplerIndex(String name) {
        int n = AL10.alGetInteger(SOFTSourceResampler.AL_NUM_RESAMPLERS_SOFT);
        for (int i = 0; i < n; i++) {
            if (name.equalsIgnoreCase(SOFTSourceResampler.alGetStringiSOFT(
                    SOFTSourceResampler.AL_RESAMPLER_NAME_SOFT, i))) {
                return i;
            }
        }
        return -1;
    }

    static float[] render(int frames) {
        scratch.clear();
        SOFTLoopback.alcRenderSamplesSOFT(device, scratch, frames);
        float[] out = new float[frames * 2];
        scratch.get(out, 0, frames * 2);
        return out;
    }

    static double rms(float[] out, int ch) {
        double acc = 0.0;
        int n = out.length / 2;
        for (int i = 0; i < n; i++) {
            double v = out[i * 2 + ch];
            acc += v * v;
        }
        return Math.sqrt(acc / n);
    }

    static double peak(float[] out, int ch) {
        double m = 0.0;
        for (int i = 0; i < out.length / 2; i++) {
            m = Math.max(m, Math.abs(out[i * 2 + ch]));
        }
        return m;
    }

    /// Energy away from the fundamental over energy at it, in dB, on the left
    /// channel of the first [`DFT_N`] frames.
    ///
    /// Delay-invariant - a magnitude spectrum does not care where a filter put
    /// its group delay - which is why it is this and not a sample-by-sample
    /// difference. OpenAL's higher-order resamplers delay the signal and a
    /// linear interpolator does not, so a direct difference would measure the
    /// delay rather than the filter shape.
    static double distortionToSignalDb(float[] out, double fundamentalHz) {
        int n = DFT_N;
        double[] x = new double[n];
        for (int i = 0; i < n; i++) {
            // 4-term Blackman-Harris rather than Hann, and the difference is
            // not cosmetic: under Hann the summed sidelobe leakage of the
            // fundamental floored this statistic at about -46 dB, which is
            // exactly where the default resampler's rows landed - so every one
            // of them was reading the WINDOW rather than the resampler, and the
            // default-versus-linear gaps were lower bounds without saying so.
            // Blackman-Harris sidelobes are near -92 dB and put the floor well
            // below anything measured here; the degenerate pitch-2.0 row, where
            // no interpolation happens at all, reads it directly.
            double t = 2.0 * Math.PI * i / n;
            double w = 0.35875 - 0.48829 * Math.cos(t) + 0.14128 * Math.cos(2.0 * t)
                - 0.01168 * Math.cos(3.0 * t);
            x[i] = out[i * 2] * w;
        }
        double bin = fundamentalHz * n / RATE;
        int lo = (int) Math.max(0.0, Math.floor(bin) - FUND_HALFWIDTH);
        int hi = (int) Math.min(n / 2.0 - 1.0, Math.ceil(bin) + FUND_HALFWIDTH);
        double fund = 0.0, total = 0.0;
        for (int k = 0; k < n / 2; k++) {
            double sr = 0.0, si = 0.0;
            for (int i = 0; i < n; i++) {
                double a = -2.0 * Math.PI * k * i / n;
                sr += x[i] * Math.cos(a);
                si += x[i] * Math.sin(a);
            }
            double m2 = sr * sr + si * si;
            total += m2;
            if (k >= lo && k <= hi) {
                fund += m2;
            }
        }
        double rest = Math.max(total - fund, 0.0);
        if (fund <= 0.0) {
            return Double.NaN;
        }
        return 10.0 * Math.log10(rest / fund + 1.0e-300);
    }

    static void p(String s) {
        System.out.println(s);
    }

    static String fmtId(double v) {
        String s = String.valueOf(v);
        return s.replace('.', 'p').replace("-", "neg");
    }
}
