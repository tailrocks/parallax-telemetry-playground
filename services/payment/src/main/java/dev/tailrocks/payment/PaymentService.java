package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.AuthorizeResponse;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.CaptureResponse;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.GetPaymentResponse;
import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentGrpc;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.RefundResponse;
import dev.tailrocks.payment.v1.VoidRequest;
import dev.tailrocks.payment.v1.VoidResponse;
import io.grpc.Metadata;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.StatusCode;
import io.sentry.Sentry;
import java.util.function.Supplier;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.grpc.server.service.GrpcService;

/** gRPC adapter for the durable payment lifecycle. */
@GrpcService
class PaymentService extends PaymentGrpc.PaymentImplBase {
    private static final Logger LOG = LoggerFactory.getLogger(PaymentService.class);
    static final Metadata.Key<String> FAILURE_REASON = Metadata.Key.of(
        "payment-failure-reason",
        Metadata.ASCII_STRING_MARSHALLER
    );
    private static final Metadata.Key<String> OPERATION = Metadata.Key.of(
        "payment-operation",
        Metadata.ASCII_STRING_MARSHALLER
    );

    private final PaymentStore store;

    PaymentService(PaymentStore store) {
        this.store = store;
    }

    @Override
    public void authorize(AuthorizeRequest request, StreamObserver<AuthorizeResponse> observer) {
        respond(
            "authorize",
            observer,
            () -> {
                PaymentRequestValidator.authorize(request);
                PaymentFaultPolicy.Decision decision = PaymentFaultPolicy.forMethod(
                    request.getPaymentMethod()
                );
                if (decision.isTransportFailure()) {
                    // Synthetic provider faults model failure before a provider decision. Keep
                    // this transport-level: no payment resource or durable operation row exists,
                    // so callers must treat UNAVAILABLE as retryable rather than compensating an
                    // unknown payment.
                    Span.current().setAttribute("payment.provider.outcome", "transport_failure");
                    Span.current().setAttribute("payment.durable_side_effect", "none");
                    throw decision.transportException();
                }
                return store.authorize(request, decision);
            }
        );
    }

    @Override
    public void capture(CaptureRequest request, StreamObserver<CaptureResponse> observer) {
        respond(
            "capture",
            observer,
            () -> {
                PaymentRequestValidator.capture(request);
                return store.capture(request);
            }
        );
    }

    @Override
    public void void_(VoidRequest request, StreamObserver<VoidResponse> observer) {
        respond(
            "void",
            observer,
            () -> {
                PaymentRequestValidator.voidPayment(request);
                return store.voidPayment(request);
            }
        );
    }

    @Override
    public void refund(RefundRequest request, StreamObserver<RefundResponse> observer) {
        respond(
            "refund",
            observer,
            () -> {
                PaymentRequestValidator.refund(request);
                return store.refund(request);
            }
        );
    }

    @Override
    public void getPayment(GetPaymentRequest request, StreamObserver<GetPaymentResponse> observer) {
        respond(
            "get_payment",
            observer,
            () -> {
                PaymentRequestValidator.getPayment(request);
                return store.getPayment(request);
            }
        );
    }

    private <T> void respond(String operation, StreamObserver<T> observer, Supplier<T> action) {
        Span.current().setAttribute("payment.operation", operation);
        try {
            T response = action.get();
            Span.current().setAttribute("payment.operation.status", operationStatus(response));
            observer.onNext(response);
            observer.onCompleted();
        } catch (PaymentRpcException error) {
            recordFailure(operation, error, false);
            observer.onError(toStatusException(operation, error));
        } catch (RuntimeException error) {
            PaymentRpcException internal = new PaymentRpcException(
                Status.Code.INTERNAL,
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "payment persistence failed",
                error
            );
            recordFailure(operation, internal, true);
            Sentry.captureException(error);
            observer.onError(toStatusException(operation, internal));
        }
    }

    private static String operationStatus(Object response) {
        if (response instanceof AuthorizeResponse value) {
            return value.getOperationStatus().name();
        }
        if (response instanceof CaptureResponse value) {
            return value.getOperationStatus().name();
        }
        if (response instanceof VoidResponse value) {
            return value.getOperationStatus().name();
        }
        if (response instanceof RefundResponse value) {
            return value.getOperationStatus().name();
        }
        if (response instanceof GetPaymentResponse value) {
            return value.getStatus().name();
        }
        return "unknown";
    }

    private static void recordFailure(
        String operation,
        PaymentRpcException error,
        boolean unexpected
    ) {
        Span.current().recordException(error);
        Span.current().setStatus(StatusCode.ERROR, error.getMessage());
        Throwable cause = unexpected && error.getCause() != null ? error.getCause() : error;
        LOG.atError()
            .addKeyValue("payment.operation", operation)
            .addKeyValue("payment.status", error.statusCode().name())
            .addKeyValue("payment.failure_reason", error.failureReason().name())
            .setCause(cause)
            .log("payment operation failed");
    }

    private static io.grpc.StatusRuntimeException toStatusException(
        String operation,
        PaymentRpcException error
    ) {
        Metadata metadata = new Metadata();
        metadata.put(FAILURE_REASON, error.failureReason().name());
        metadata.put(OPERATION, operation);
        Status status = Status.fromCode(error.statusCode())
            .withDescription(error.getMessage());
        if (error.getCause() != null) {
            status = status.withCause(error.getCause());
        }
        return status.asRuntimeException(metadata);
    }
}
